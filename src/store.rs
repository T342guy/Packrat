// SPDX-License-Identifier: GPL-3.0-only
//! All SQL lives here. Handlers stay thin; this module owns the data model
//! invariants (label codes, container nesting, tag normalisation).

use crate::error::{AppError, AppResult};
use crate::models::*;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Crockford-ish alphabet: no 0/O/1/I/L/U, so codes survive being read off a
/// label by a human and typed into the search box.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
const MAX_DEPTH: usize = 64;

pub const KINDS: &[&str] = &[
    "area", "shelf", "cabinet", "drawer", "bin", "box", "bag", "other",
];

/// How long a container's contents are trusted before the app suggests
/// re-checking them. Overridable in Settings.
pub const DEFAULT_STALE_AFTER_DAYS: i64 = 180;

pub fn stale_after_days(conn: &Connection) -> i64 {
    crate::db::get_setting(conn, "stale_after_days")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_STALE_AFTER_DAYS)
}

fn kind_prefix(kind: &str) -> &'static str {
    match kind {
        "area" => "AR",
        "shelf" => "SH",
        "cabinet" => "CB",
        "drawer" => "DR",
        "bin" => "BN",
        "bag" => "BG",
        "other" => "CT",
        _ => "BX",
    }
}

pub fn normalize_kind(kind: &str) -> String {
    let k = kind.trim().to_lowercase();
    if KINDS.contains(&k.as_str()) {
        k
    } else {
        "box".to_string()
    }
}

/// Generates a short, unique, human-readable label code such as `BX-7K3Q`.
fn generate_code(conn: &Connection, kind: &str) -> AppResult<String> {
    use rand::Rng;
    let prefix = kind_prefix(kind);
    for attempt in 0..40 {
        let len = if attempt < 30 { 4 } else { 6 };
        let mut rng = rand::thread_rng();
        let suffix: String = (0..len)
            .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
            .collect();
        let code = format!("{prefix}-{suffix}");
        let taken: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM containers WHERE code = ?1)",
            [&code],
            |r| r.get(0),
        )?;
        if !taken {
            return Ok(code);
        }
    }
    Err(AppError::internal("could not allocate a unique label code"))
}

/// Barcodes are stored trimmed; blank means "none".
pub fn normalize_barcode(barcode: Option<&str>) -> Option<String> {
    barcode
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| b.to_string())
}

/// Refuses a barcode already used elsewhere, so one scan means one thing.
fn check_barcode_free(
    conn: &Connection,
    table: &str,
    barcode: &Option<String>,
    exclude_id: Option<i64>,
) -> AppResult<()> {
    let Some(value) = barcode else { return Ok(()) };
    let sql = format!(
        "SELECT name FROM {table} WHERE barcode = ?1 COLLATE NOCASE AND id IS NOT ?2 LIMIT 1"
    );
    let clash: Option<String> = conn
        .query_row(&sql, params![value, exclude_id], |r| r.get(0))
        .optional()?;
    match clash {
        Some(name) => Err(AppError::bad_request(format!(
            "barcode {value} is already assigned to {name}"
        ))),
        None => Ok(()),
    }
}

pub fn normalize_code(code: &str) -> String {
    code.trim().to_uppercase().replace(' ', "-")
}

// --------------------------------------------------------------------- clock
//
// Staleness is measured against the operating system's clock, which is the
// only sense of time a process gets across restarts. That is fine for the
// ordinary case — stopping Packrat for six months does not hide the six
// months, because the clock keeps running without it. It is not fine when the
// clock itself is wrong: a machine with no battery-backed clock can boot in
// 1970, and a clock that jumps backwards would make everything look freshly
// checked.
//
// So the latest time ever observed is recorded, and time is read as "now, or
// the high-water mark, whichever is later". A clock that moves backwards then
// freezes the ages where they were instead of winding them back, and the
// discrepancy is reported rather than silently absorbed.

const CLOCK_KEY: &str = "clock_high_water";
/// Small backwards steps are normal — NTP corrections, leap smearing.
const CLOCK_TOLERANCE_SECONDS: i64 = 120;

#[derive(Debug, Serialize)]
pub struct ClockStatus {
    pub now: String,
    pub high_water: Option<String>,
    /// How far behind the recorded high-water mark the system clock is, when
    /// that gap is big enough to matter.
    pub behind_seconds: Option<i64>,
}

/// The current time, never earlier than the latest time already seen.
pub fn trusted_now(conn: &Connection) -> AppResult<String> {
    Ok(conn.query_row(
        "SELECT MAX(datetime('now'),
                    COALESCE((SELECT value FROM settings WHERE key = ?1), datetime('now')))",
        [CLOCK_KEY],
        |r| r.get(0),
    )?)
}

/// Records that time has reached at least this point.
pub fn touch_clock(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = MAX(value, excluded.value)",
        [CLOCK_KEY],
    )?;
    Ok(())
}

pub fn clock_status(conn: &Connection) -> AppResult<ClockStatus> {
    let (now, high_water): (String, Option<String>) = conn.query_row(
        "SELECT datetime('now'), (SELECT value FROM settings WHERE key = ?1)",
        [CLOCK_KEY],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let behind_seconds = match &high_water {
        Some(mark) => {
            let gap: i64 = conn.query_row(
                "SELECT CAST((julianday(?1) - julianday(?2)) * 86400 AS INTEGER)",
                [mark, &now],
                |r| r.get(0),
            )?;
            (gap > CLOCK_TOLERANCE_SECONDS).then_some(gap)
        }
        None => None,
    };
    Ok(ClockStatus {
        now,
        high_water,
        behind_seconds,
    })
}

// ---------------------------------------------------------------- containers

struct RawContainer {
    id: i64,
    code: String,
    name: String,
    kind: String,
    parent_id: Option<i64>,
    notes: String,
    photo_id: Option<i64>,
    barcode: Option<String>,
    created_at: String,
    updated_at: String,
    checked_at: Option<String>,
    age_seconds: i64,
}

fn load_raw(conn: &Connection) -> AppResult<Vec<RawContainer>> {
    let now = trusted_now(conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, code, name, kind, parent_id, notes, photo_id, barcode, created_at, updated_at,
                checked_at,
                CAST((julianday(?1) - julianday(COALESCE(checked_at, created_at))) * 86400
                     AS INTEGER)
         FROM containers",
    )?;
    let rows = stmt.query_map([&now], |r| {
        Ok(RawContainer {
            id: r.get(0)?,
            code: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            parent_id: r.get(4)?,
            notes: r.get(5)?,
            photo_id: r.get(6)?,
            barcode: r.get(7)?,
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
            checked_at: r.get(10)?,
            age_seconds: r.get::<_, Option<i64>>(11)?.unwrap_or(0).max(0),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Builds "Garage / Shelf / Box" paths and depths, guarding against cycles a
/// hand-edited database could contain.
fn build_paths(rows: &[(i64, String, Option<i64>)]) -> HashMap<i64, (String, i64)> {
    let index: HashMap<i64, usize> = rows.iter().enumerate().map(|(i, r)| (r.0, i)).collect();
    let mut out = HashMap::with_capacity(rows.len());
    for (id, name, parent) in rows {
        let mut names = vec![name.clone()];
        let mut seen = HashSet::from([*id]);
        let mut cursor = *parent;
        while let Some(parent_id) = cursor {
            if !seen.insert(parent_id) || names.len() > MAX_DEPTH {
                break;
            }
            match index.get(&parent_id) {
                Some(&i) => {
                    names.push(rows[i].1.clone());
                    cursor = rows[i].2;
                }
                None => break,
            }
        }
        let depth = names.len() as i64 - 1;
        names.reverse();
        out.insert(*id, (names.join(" / "), depth));
    }
    out
}

/// Just the id-to-path map. Item queries need paths but not content counts,
/// and the counts cost a group-by over every item in the database.
fn container_paths(conn: &Connection) -> AppResult<HashMap<i64, String>> {
    let mut stmt = conn.prepare("SELECT id, name, parent_id FROM containers")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<(i64, String, Option<i64>)>>>()?;
    Ok(build_paths(&rows)
        .into_iter()
        .map(|(id, (path, _))| (id, path))
        .collect())
}

/// Loads every container, enriched with its full path and content counts.
/// Containers are few (tens to hundreds even in a very full garage), so doing
/// the tree work in Rust is simpler and faster than recursive SQL.
pub fn all_containers(conn: &Connection) -> AppResult<Vec<Container>> {
    let raw = load_raw(conn)?;
    let threshold = stale_after_days(conn);

    let mut direct_items: HashMap<i64, (i64, i64)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT container_id, COUNT(*), COALESCE(SUM(quantity), 0)
             FROM items WHERE container_id IS NOT NULL GROUP BY container_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (cid, count, qty) = row?;
            direct_items.insert(cid, (count, qty));
        }
    }

    let mut child_counts: HashMap<i64, i64> = HashMap::new();
    for c in &raw {
        if let Some(p) = c.parent_id {
            *child_counts.entry(p).or_insert(0) += 1;
        }
    }

    let paths = build_paths(
        &raw.iter()
            .map(|c| (c.id, c.name.clone(), c.parent_id))
            .collect::<Vec<_>>(),
    );

    let mut out: Vec<Container> = raw
        .iter()
        .map(|c| {
            let (path, depth) = paths.get(&c.id).cloned().unwrap_or((c.name.clone(), 0));
            let (item_count, total_quantity) = direct_items.get(&c.id).copied().unwrap_or((0, 0));
            Container {
                id: c.id,
                code: c.code.clone(),
                name: c.name.clone(),
                kind: c.kind.clone(),
                parent_id: c.parent_id,
                notes: c.notes.clone(),
                photo_id: c.photo_id,
                barcode: c.barcode.clone(),
                created_at: c.created_at.clone(),
                updated_at: c.updated_at.clone(),
                checked_at: c.checked_at.clone(),
                days_since_check: c.checked_at.as_ref().map(|_| c.age_seconds / 86_400),
                seconds_since_check: c.checked_at.as_ref().map(|_| c.age_seconds),
                age_days: c.age_seconds / 86_400,
                age_seconds: c.age_seconds,
                // Only containers actually holding something can go stale:
                // an empty shelf has nothing to verify.
                stale: item_count > 0 && c.age_seconds / 86_400 > threshold,
                path,
                depth,
                item_count,
                total_quantity,
                child_count: child_counts.get(&c.id).copied().unwrap_or(0),
            }
        })
        .collect();

    out.sort_by_key(|c| c.path.to_lowercase());
    Ok(out)
}

pub fn container_by_id(conn: &Connection, id: i64) -> AppResult<Container> {
    all_containers(conn)?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::not_found(format!("no container with id {id}")))
}

pub fn container_by_code(conn: &Connection, code: &str) -> AppResult<Container> {
    let wanted = normalize_code(code);
    all_containers(conn)?
        .into_iter()
        .find(|c| c.code.to_uppercase() == wanted)
        .ok_or_else(|| AppError::not_found(format!("no container with code {code}")))
}

/// Ids of `root` and everything nested underneath it.
pub fn descendant_ids(all: &[Container], root: i64) -> Vec<i64> {
    let mut ids = vec![root];
    let mut frontier = vec![root];
    while let Some(current) = frontier.pop() {
        for c in all {
            if c.parent_id == Some(current) && !ids.contains(&c.id) {
                ids.push(c.id);
                frontier.push(c.id);
            }
        }
    }
    ids
}

fn validate_parent(all: &[Container], id: Option<i64>, parent_id: Option<i64>) -> AppResult<()> {
    let Some(parent) = parent_id else {
        return Ok(());
    };
    if !all.iter().any(|c| c.id == parent) {
        return Err(AppError::bad_request("parent container does not exist"));
    }
    if let Some(id) = id {
        if parent == id {
            return Err(AppError::bad_request("a container cannot contain itself"));
        }
        if descendant_ids(all, id).contains(&parent) {
            return Err(AppError::bad_request(
                "cannot move a container inside one of its own contents",
            ));
        }
    }
    Ok(())
}

pub fn create_container(conn: &Connection, input: &ContainerInput) -> AppResult<Container> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    let kind = normalize_kind(&input.kind);
    let all = all_containers(conn)?;
    validate_parent(&all, None, input.parent_id)?;

    let code = match input.code.as_deref().map(normalize_code) {
        Some(c) if !c.is_empty() => {
            if all.iter().any(|x| x.code.to_uppercase() == c) {
                return Err(AppError::bad_request(format!(
                    "label code {c} is already in use"
                )));
            }
            c
        }
        _ => generate_code(conn, &kind)?,
    };

    let barcode = normalize_barcode(input.barcode.as_deref());
    check_barcode_free(conn, "containers", &barcode, None)?;
    conn.execute(
        "INSERT INTO containers (code, name, kind, parent_id, notes, photo_id, barcode)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            code,
            name,
            kind,
            input.parent_id,
            input.notes.trim(),
            input.photo_id,
            barcode
        ],
    )?;
    container_by_id(conn, conn.last_insert_rowid())
}

pub fn update_container(
    conn: &Connection,
    id: i64,
    input: &ContainerInput,
) -> AppResult<Container> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    let kind = normalize_kind(&input.kind);
    let all = all_containers(conn)?;
    if !all.iter().any(|c| c.id == id) {
        return Err(AppError::not_found(format!("no container with id {id}")));
    }
    validate_parent(&all, Some(id), input.parent_id)?;

    let code = match input.code.as_deref().map(normalize_code) {
        Some(c) if !c.is_empty() => {
            if all.iter().any(|x| x.id != id && x.code.to_uppercase() == c) {
                return Err(AppError::bad_request(format!(
                    "label code {c} is already in use"
                )));
            }
            c
        }
        _ => all
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.code.clone())
            .unwrap_or_default(),
    };

    let barcode = normalize_barcode(input.barcode.as_deref());
    check_barcode_free(conn, "containers", &barcode, Some(id))?;
    conn.execute(
        "UPDATE containers
            SET code = ?1, name = ?2, kind = ?3, parent_id = ?4, notes = ?5, photo_id = ?6,
                barcode = ?7, updated_at = datetime('now')
          WHERE id = ?8",
        params![
            code,
            name,
            kind,
            input.parent_id,
            input.notes.trim(),
            input.photo_id,
            barcode,
            id
        ],
    )?;
    container_by_id(conn, id)
}

/// Deletes a container. Its child containers are lifted up to its parent and
/// its items become unfiled — deleting a box must never delete belongings.
pub fn delete_container(conn: &mut Connection, id: i64) -> AppResult<()> {
    let tx = conn.transaction()?;
    let parent: Option<i64> = tx
        .query_row(
            "SELECT parent_id FROM containers WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("no container with id {id}")))?;
    tx.execute(
        "UPDATE containers SET parent_id = ?1 WHERE parent_id = ?2",
        params![parent, id],
    )?;
    tx.execute(
        "UPDATE items SET container_id = NULL WHERE container_id = ?1",
        [id],
    )?;
    tx.execute("DELETE FROM containers WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}

/// Records that someone has just eyeballed this container's contents.
pub fn mark_checked(conn: &Connection, id: i64) -> AppResult<Container> {
    touch_clock(conn)?;
    let changed = conn.execute(
        "UPDATE containers SET checked_at = datetime('now') WHERE id = ?1",
        [id],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("no container with id {id}")));
    }
    container_by_id(conn, id)
}

pub fn container_detail(conn: &Connection, id: i64) -> AppResult<ContainerDetail> {
    let all = all_containers(conn)?;
    let container = all
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or_else(|| AppError::not_found(format!("no container with id {id}")))?;

    let mut ancestors = Vec::new();
    let mut cursor = container.parent_id;
    let mut seen = HashSet::from([id]);
    while let Some(pid) = cursor {
        if !seen.insert(pid) {
            break;
        }
        match all.iter().find(|c| c.id == pid) {
            Some(c) => {
                ancestors.push(c.clone());
                cursor = c.parent_id;
            }
            None => break,
        }
    }
    ancestors.reverse();

    // Each child is returned with its own contents so a shelf can show every
    // box on it with a collapsed list of what's inside. One query covers this
    // container and all of its children: querying per child re-scanned the tag
    // table and rebuilt the container tree once per box.
    let child_list: Vec<&Container> = all.iter().filter(|c| c.parent_id == Some(id)).collect();
    let mut wanted: Vec<i64> = vec![id];
    wanted.extend(child_list.iter().map(|c| c.id));
    let loaded = query_items(
        conn,
        &ItemQuery {
            container_ids: Some(wanted),
            ..Default::default()
        },
    )?;

    let mut by_container: HashMap<i64, Vec<Item>> = HashMap::new();
    for item in loaded {
        if let Some(cid) = item.container_id {
            by_container.entry(cid).or_default().push(item);
        }
    }
    let children: Vec<ChildNode> = child_list
        .iter()
        .map(|child| ChildNode {
            child_count: child.child_count,
            container: (*child).clone(),
            items: by_container.remove(&child.id).unwrap_or_default(),
        })
        .collect();

    let items = by_container.remove(&id).unwrap_or_default();

    let nested = descendant_ids(&all, id);
    let (nested_item_count, nested_total_quantity) = all
        .iter()
        .filter(|c| nested.contains(&c.id))
        .fold((0, 0), |(n, q), c| (n + c.item_count, q + c.total_quantity));

    Ok(ContainerDetail {
        container,
        ancestors,
        children,
        items,
        nested_item_count,
        nested_total_quantity,
    })
}

// --------------------------------------------------------------------- items

#[derive(Debug, Default)]
pub struct ItemQuery {
    pub q: Option<String>,
    pub container_id: Option<i64>,
    /// Fetch the contents of several containers at once, so a shelf can load
    /// every box on it in one query instead of one query per box.
    pub container_ids: Option<Vec<i64>>,
    /// Restrict to specific item ids.
    pub ids: Option<Vec<i64>>,
    pub include_nested: bool,
    pub tag: Option<String>,
    pub unfiled: bool,
    pub sort: Option<String>,
    pub limit: Option<i64>,
}

/// Escapes a user search term for use inside a `LIKE ... ESCAPE '\'` pattern.
fn like_pattern(term: &str) -> String {
    let escaped = term
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn search_terms(q: &str) -> Vec<String> {
    q.split_whitespace()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .take(8)
        .collect()
}

pub fn query_items(conn: &Connection, query: &ItemQuery) -> AppResult<Vec<Item>> {
    let mut sql = String::from(
        "SELECT i.id, i.name, i.description, i.quantity, i.container_id, i.photo_id,
                i.barcode, i.created_at, i.updated_at, c.code, c.name
           FROM items i
           LEFT JOIN containers c ON c.id = i.container_id
          WHERE 1 = 1",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ids) = &query.ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        sql.push_str(&format!(" AND i.id IN ({placeholders})"));
        for id in ids {
            binds.push(Box::new(*id));
        }
    }
    if let Some(ids) = &query.container_ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        sql.push_str(&format!(" AND i.container_id IN ({placeholders})"));
        for id in ids {
            binds.push(Box::new(*id));
        }
    }
    if let Some(cid) = query.container_id {
        if query.include_nested {
            let all = all_containers(conn)?;
            let ids = descendant_ids(&all, cid);
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            sql.push_str(&format!(" AND i.container_id IN ({placeholders})"));
            for id in ids {
                binds.push(Box::new(id));
            }
        } else {
            sql.push_str(" AND i.container_id = ?");
            binds.push(Box::new(cid));
        }
    }
    if query.unfiled {
        sql.push_str(" AND i.container_id IS NULL");
    }
    if let Some(tag) = &query.tag {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM item_tags it JOIN tags t ON t.id = it.tag_id
                           WHERE it.item_id = i.id AND t.name = ? COLLATE NOCASE)",
        );
        binds.push(Box::new(tag.trim().to_string()));
    }

    let terms = query.q.as_deref().map(search_terms).unwrap_or_default();
    for term in &terms {
        // Every term must match somewhere: the item, its tags, or the label of
        // the container it lives in (so "camping BX-7K3Q" narrows correctly).
        sql.push_str(
            " AND (i.name LIKE ? ESCAPE '\\' OR i.description LIKE ? ESCAPE '\\'
                   OR c.name LIKE ? ESCAPE '\\' OR c.code LIKE ? ESCAPE '\\'
                   OR EXISTS (SELECT 1 FROM item_tags it JOIN tags t ON t.id = it.tag_id
                               WHERE it.item_id = i.id AND t.name LIKE ? ESCAPE '\\'))",
        );
        let pattern = like_pattern(term);
        for _ in 0..5 {
            binds.push(Box::new(pattern.clone()));
        }
    }

    match query.sort.as_deref() {
        Some("newest") => sql.push_str(" ORDER BY i.created_at DESC, i.id DESC"),
        Some("updated") => sql.push_str(" ORDER BY i.updated_at DESC, i.id DESC"),
        Some("quantity") => sql.push_str(" ORDER BY i.quantity DESC, i.name COLLATE NOCASE"),
        _ => sql.push_str(" ORDER BY i.name COLLATE NOCASE, i.id"),
    }
    if let Some(limit) = query.limit {
        sql.push_str(&format!(" LIMIT {}", limit.clamp(1, 10_000)));
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
        Ok(Item {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            quantity: r.get(3)?,
            container_id: r.get(4)?,
            photo_id: r.get(5)?,
            barcode: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
            tags: Vec::new(),
            container_code: r.get(9)?,
            container_name: r.get(10)?,
            container_path: None,
        })
    })?;
    let mut items: Vec<Item> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    attach_tags(conn, &mut items)?;
    attach_paths(conn, &mut items)?;

    // Relevance ranking happens in Rust: SQL got us the candidate set, but a
    // name hit should always outrank a passing mention in a description.
    if !terms.is_empty() {
        let mut scored: Vec<(i64, Item)> = items
            .into_iter()
            .map(|i| (score_item(&i, &terms), i))
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        });
        items = scored.into_iter().map(|(_, i)| i).collect();
    }
    Ok(items)
}

fn score_item(item: &Item, terms: &[String]) -> i64 {
    let name = item.name.to_lowercase();
    let description = item.description.to_lowercase();
    let tags: Vec<String> = item.tags.iter().map(|t| t.to_lowercase()).collect();
    let container = format!(
        "{} {}",
        item.container_name.clone().unwrap_or_default(),
        item.container_code.clone().unwrap_or_default()
    )
    .to_lowercase();

    let mut score = 0;
    for term in terms {
        if name == *term {
            score += 120;
        } else if name.starts_with(term) {
            score += 70;
        } else if name
            .split_whitespace()
            .any(|w| w.starts_with(term.as_str()))
        {
            score += 55;
        } else if name.contains(term) {
            score += 40;
        }
        if tags.iter().any(|t| t == term) {
            score += 35;
        } else if tags.iter().any(|t| t.contains(term)) {
            score += 20;
        }
        if container.contains(term) {
            score += 15;
        }
        if description.contains(term) {
            score += 10;
        }
    }
    score
}

fn attach_tags(conn: &Connection, items: &mut [Item]) -> AppResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    // Scoped to these items: reading every tag link in the database to decorate
    // a single scanned item was the difference between a scan costing under a
    // millisecond and costing tens.
    let placeholders = items.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT it.item_id, t.name FROM item_tags it
         JOIN tags t ON t.id = it.tag_id
         WHERE it.item_id IN ({placeholders})
         ORDER BY t.name COLLATE NOCASE"
    ))?;
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let rows = stmt.query_map(params_from_iter(ids.iter()), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (item_id, tag) = row?;
        map.entry(item_id).or_default().push(tag);
    }
    for item in items.iter_mut() {
        if let Some(tags) = map.remove(&item.id) {
            item.tags = tags;
        }
    }
    Ok(())
}

fn attach_paths(conn: &Connection, items: &mut [Item]) -> AppResult<()> {
    if !items.iter().any(|i| i.container_id.is_some()) {
        return Ok(());
    }
    let paths = container_paths(conn)?;
    for item in items.iter_mut() {
        if let Some(cid) = item.container_id {
            item.container_path = paths.get(&cid).cloned();
        }
    }
    Ok(())
}

pub fn item_by_id(conn: &Connection, id: i64) -> AppResult<Item> {
    // Fetch the one row. This used to load the whole items table and filter in
    // Rust, so every scan, edit and quantity change cost a full table walk plus
    // a scan of every tag link.
    let mut items = query_items(
        conn,
        &ItemQuery {
            ids: Some(vec![id]),
            ..Default::default()
        },
    )?;
    if items.is_empty() {
        return Err(AppError::not_found(format!("no item with id {id}")));
    }
    Ok(items.remove(0))
}

pub fn create_item(conn: &mut Connection, input: &ItemInput) -> AppResult<Item> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    if let Some(cid) = input.container_id {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM containers WHERE id = ?1)",
            [cid],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(AppError::bad_request("container does not exist"));
        }
    }
    let barcode = normalize_barcode(input.barcode.as_deref());
    check_barcode_free(conn, "items", &barcode, None)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO items (name, description, quantity, container_id, photo_id, barcode)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            name,
            input.description.trim(),
            input.quantity.max(0),
            input.container_id,
            input.photo_id,
            barcode
        ],
    )?;
    let id = tx.last_insert_rowid();
    set_tags(&tx, id, &input.tags)?;
    tx.commit()?;
    item_by_id(conn, id)
}

pub fn update_item(conn: &mut Connection, id: i64, input: &ItemInput) -> AppResult<Item> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    let barcode = normalize_barcode(input.barcode.as_deref());
    check_barcode_free(conn, "items", &barcode, Some(id))?;
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE items
            SET name = ?1, description = ?2, quantity = ?3, container_id = ?4, photo_id = ?5,
                barcode = ?6, updated_at = datetime('now')
          WHERE id = ?7",
        params![
            name,
            input.description.trim(),
            input.quantity.max(0),
            input.container_id,
            input.photo_id,
            barcode,
            id
        ],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("no item with id {id}")));
    }
    set_tags(&tx, id, &input.tags)?;
    tx.commit()?;
    item_by_id(conn, id)
}

pub fn delete_item(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute("DELETE FROM items WHERE id = ?1", [id])?;
    if changed == 0 {
        return Err(AppError::not_found(format!("no item with id {id}")));
    }
    prune_tags(conn)?;
    Ok(())
}

pub fn move_item(conn: &Connection, id: i64, container_id: Option<i64>) -> AppResult<Item> {
    if let Some(cid) = container_id {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM containers WHERE id = ?1)",
            [cid],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(AppError::bad_request("container does not exist"));
        }
    }
    let changed = conn.execute(
        "UPDATE items SET container_id = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![container_id, id],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("no item with id {id}")));
    }
    item_by_id(conn, id)
}

pub fn adjust_quantity(conn: &Connection, id: i64, delta: i64) -> AppResult<Item> {
    let changed = conn.execute(
        "UPDATE items SET quantity = MAX(0, quantity + ?1), updated_at = datetime('now')
         WHERE id = ?2",
        params![delta, id],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("no item with id {id}")));
    }
    item_by_id(conn, id)
}

// ---------------------------------------------------------------------- tags

fn set_tags(conn: &Connection, item_id: i64, tags: &[String]) -> AppResult<()> {
    // Remember what this item was tagged with: only those tags can be orphaned
    // by this change, so only those need checking afterwards.
    let previous: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT tag_id FROM item_tags WHERE item_id = ?1")?;
        let rows = stmt.query_map([item_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<Vec<i64>>>()?
    };
    conn.execute("DELETE FROM item_tags WHERE item_id = ?1", [item_id])?;
    let mut seen: HashSet<String> = HashSet::new();
    for raw in tags {
        let tag = raw.trim();
        if tag.is_empty() || tag.len() > 64 {
            continue;
        }
        if !seen.insert(tag.to_lowercase()) {
            continue;
        }
        conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
        let tag_id: i64 = conn.query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            [tag],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
            params![item_id, tag_id],
        )?;
    }
    // Sweeping the whole tag table here made saving one item cost a scan of
    // every tag link in the database.
    for tag_id in previous {
        conn.execute(
            "DELETE FROM tags WHERE id = ?1
              AND NOT EXISTS (SELECT 1 FROM item_tags WHERE tag_id = ?1)",
            [tag_id],
        )?;
    }
    Ok(())
}

fn prune_tags(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM item_tags)",
        [],
    )?;
    Ok(())
}

pub fn all_tags(conn: &Connection) -> AppResult<Vec<TagCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(it.item_id) AS n
           FROM tags t LEFT JOIN item_tags it ON it.tag_id = t.id
          GROUP BY t.id
          ORDER BY n DESC, t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagCount {
            name: r.get(0)?,
            item_count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Renames a tag everywhere it is used. Renaming onto an existing tag merges
/// the two rather than failing.
pub fn rename_tag(conn: &mut Connection, old: &str, new: &str) -> AppResult<String> {
    let new = new.trim();
    if new.is_empty() {
        return Err(AppError::bad_request("the new tag name cannot be empty"));
    }
    if new.len() > 64 {
        return Err(AppError::bad_request(
            "tag names are limited to 64 characters",
        ));
    }
    let tx = conn.transaction()?;
    let old_id: i64 = tx
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            [old],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("no tag called {old}")))?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            [new],
            |r| r.get(0),
        )
        .optional()?;

    match existing {
        Some(target) if target != old_id => {
            tx.execute(
                "INSERT OR IGNORE INTO item_tags (item_id, tag_id)
                 SELECT item_id, ?1 FROM item_tags WHERE tag_id = ?2",
                params![target, old_id],
            )?;
            tx.execute("DELETE FROM item_tags WHERE tag_id = ?1", [old_id])?;
            tx.execute("DELETE FROM tags WHERE id = ?1", [old_id])?;
        }
        _ => {
            tx.execute(
                "UPDATE tags SET name = ?1 WHERE id = ?2",
                params![new, old_id],
            )?;
        }
    }
    tx.commit()?;
    Ok(new.to_string())
}

/// Removes a tag from every item that carries it.
pub fn delete_tag(conn: &Connection, name: &str) -> AppResult<()> {
    let changed = conn.execute(
        "DELETE FROM tags WHERE name = ?1 COLLATE NOCASE",
        [name.trim()],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("no tag called {name}")));
    }
    Ok(())
}

// ------------------------------------------------------------------ scanning

/// Works out what a scanned string refers to: one of our label codes, a
/// barcode stuck on a container, or a barcode on an item.
pub fn resolve_scan(conn: &Connection, raw: &str) -> AppResult<ScanResult> {
    let scanned = raw.trim();
    if scanned.is_empty() {
        return Err(AppError::bad_request("nothing was scanned"));
    }

    let by_code: Option<i64> = conn
        .query_row(
            "SELECT id FROM containers WHERE code = ?1 COLLATE NOCASE",
            [normalize_code(scanned)],
            |r| r.get(0),
        )
        .optional()?;
    let container_id = match by_code {
        Some(id) => Some(id),
        None => conn
            .query_row(
                "SELECT id FROM containers WHERE barcode = ?1 COLLATE NOCASE",
                [scanned],
                |r| r.get(0),
            )
            .optional()?,
    };
    if let Some(id) = container_id {
        return Ok(ScanResult::Container {
            container: Box::new(container_detail(conn, id)?),
        });
    }

    let item_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM items WHERE barcode = ?1 COLLATE NOCASE",
            [scanned],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = item_id {
        return Ok(ScanResult::Item {
            item: Box::new(item_by_id(conn, id)?),
        });
    }

    Ok(ScanResult::Unknown {
        code: scanned.to_string(),
    })
}

// --------------------------------------------------------------------- stats

pub fn stats(conn: &Connection) -> AppResult<Stats> {
    let one = |sql: &str| -> AppResult<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    let stale_containers = all_containers(conn)?.iter().filter(|c| c.stale).count() as i64;
    Ok(Stats {
        stale_containers,
        items: one("SELECT COUNT(*) FROM items")?,
        total_quantity: one("SELECT COALESCE(SUM(quantity), 0) FROM items")?,
        containers: one("SELECT COUNT(*) FROM containers")?,
        boxes: one("SELECT COUNT(*) FROM containers WHERE kind IN ('box','bin','bag','drawer')")?,
        tags: one("SELECT COUNT(*) FROM tags")?,
        photos: one("SELECT COUNT(*) FROM photos")?,
        unfiled_items: one("SELECT COUNT(*) FROM items WHERE container_id IS NULL")?,
        empty_containers: one("SELECT COUNT(*) FROM containers c
              WHERE NOT EXISTS (SELECT 1 FROM items i WHERE i.container_id = c.id)
                AND NOT EXISTS (SELECT 1 FROM containers x WHERE x.parent_id = c.id)")?,
        database_bytes: one(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ContainerInput, ItemInput};

    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        crate::db::migrate(&mut conn).unwrap();
        conn
    }

    fn container(conn: &Connection, name: &str, kind: &str, parent: Option<i64>) -> Container {
        create_container(
            conn,
            &ContainerInput {
                name: name.into(),
                kind: kind.into(),
                parent_id: parent,
                notes: String::new(),
                photo_id: None,
                code: None,
                barcode: None,
            },
        )
        .unwrap()
    }

    fn item(
        conn: &mut Connection,
        name: &str,
        description: &str,
        container: Option<i64>,
        tags: &[&str],
    ) -> Item {
        create_item(
            conn,
            &ItemInput {
                name: name.into(),
                description: description.into(),
                quantity: 1,
                container_id: container,
                photo_id: None,
                tags: tags.iter().map(|t| t.to_string()).collect(),
                barcode: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn generated_codes_are_prefixed_by_kind_and_unique() {
        let conn = test_db();
        let a = container(&conn, "Garage", "area", None);
        let b = container(&conn, "Bits", "drawer", None);
        assert!(a.code.starts_with("AR-"), "{}", a.code);
        assert!(b.code.starts_with("DR-"), "{}", b.code);
        assert_ne!(a.code, b.code);
        // The alphabet avoids characters that are misread off a printed label.
        assert!(!a.code[3..].contains(['0', 'O', '1', 'I', 'L', 'U']));
    }

    #[test]
    fn duplicate_codes_are_rejected() {
        let conn = test_db();
        let first = container(&conn, "Garage", "area", None);
        let clash = create_container(
            &conn,
            &ContainerInput {
                name: "Other".into(),
                kind: "box".into(),
                parent_id: None,
                notes: String::new(),
                photo_id: None,
                barcode: None,
                code: Some(first.code.to_lowercase()),
            },
        );
        assert!(clash.is_err(), "codes must be unique regardless of case");
    }

    #[test]
    fn nesting_builds_readable_paths() {
        let conn = test_db();
        let garage = container(&conn, "Garage", "area", None);
        let shelf = container(&conn, "North shelves", "shelf", Some(garage.id));
        let bin = container(&conn, "Camping", "box", Some(shelf.id));
        let loaded = container_by_id(&conn, bin.id).unwrap();
        assert_eq!(loaded.path, "Garage / North shelves / Camping");
        assert_eq!(loaded.depth, 2);
    }

    #[test]
    fn a_container_cannot_be_moved_inside_itself() {
        let conn = test_db();
        let garage = container(&conn, "Garage", "area", None);
        let shelf = container(&conn, "Shelf", "shelf", Some(garage.id));
        let input = ContainerInput {
            name: "Garage".into(),
            kind: "area".into(),
            parent_id: Some(shelf.id),
            notes: String::new(),
            photo_id: None,
            code: None,
            barcode: None,
        };
        assert!(update_container(&conn, garage.id, &input).is_err());
    }

    #[test]
    fn deleting_a_container_keeps_its_contents() {
        let mut conn = test_db();
        let garage = container(&conn, "Garage", "area", None);
        let shelf = container(&conn, "Shelf", "shelf", Some(garage.id));
        let bin = container(&conn, "Bin", "bin", Some(shelf.id));
        let hammer = item(&mut conn, "Hammer", "", Some(shelf.id), &[]);

        delete_container(&mut conn, shelf.id).unwrap();

        // The bin moves up to the garage rather than being orphaned...
        assert_eq!(
            container_by_id(&conn, bin.id).unwrap().parent_id,
            Some(garage.id)
        );
        // ...and the item survives, merely unfiled.
        assert_eq!(item_by_id(&conn, hammer.id).unwrap().container_id, None);
    }

    #[test]
    fn search_requires_every_term_to_match() {
        let mut conn = test_db();
        let bin = container(&conn, "Camping gear", "box", None);
        item(&mut conn, "Camping stove", "propane", Some(bin.id), &[]);
        item(&mut conn, "Hammer", "claw hammer", None, &[]);

        let hits = query_items(
            &conn,
            &ItemQuery {
                q: Some("camping stove".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Camping stove");

        let none = query_items(
            &conn,
            &ItemQuery {
                q: Some("camping hammer".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.is_empty(), "terms are ANDed together");
    }

    #[test]
    fn search_finds_items_by_the_box_they_live_in() {
        let mut conn = test_db();
        let bin = container(&conn, "Holiday decorations", "bin", None);
        item(&mut conn, "String lights", "", Some(bin.id), &[]);

        let by_name = query_items(
            &conn,
            &ItemQuery {
                q: Some("holiday".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_name.len(), 1);

        let by_code = query_items(
            &conn,
            &ItemQuery {
                q: Some(bin.code.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_code.len(), 1);
    }

    #[test]
    fn name_matches_outrank_description_matches() {
        let mut conn = test_db();
        item(&mut conn, "Spare bulbs", "for the drill light", None, &[]);
        item(&mut conn, "Drill", "cordless", None, &[]);

        let hits = query_items(
            &conn,
            &ItemQuery {
                q: Some("drill".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            hits[0].name, "Drill",
            "the item actually called Drill should come first"
        );
    }

    #[test]
    fn like_metacharacters_are_treated_literally() {
        let mut conn = test_db();
        item(&mut conn, "Sandpaper", "", None, &[]);
        for query in ["%", "_", "%%"] {
            let hits = query_items(
                &conn,
                &ItemQuery {
                    q: Some(query.into()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(hits.is_empty(), "'{query}' must not act as a wildcard");
        }
    }

    #[test]
    fn tags_are_deduplicated_case_insensitively() {
        let mut conn = test_db();
        let saved = item(&mut conn, "Drill", "", None, &["Tools", "tools", " TOOLS "]);
        assert_eq!(saved.tags.len(), 1);
        assert_eq!(all_tags(&conn).unwrap().len(), 1);
    }

    #[test]
    fn unused_tags_are_cleaned_up() {
        let mut conn = test_db();
        let drill = item(&mut conn, "Drill", "", None, &["tools"]);
        assert_eq!(all_tags(&conn).unwrap().len(), 1);
        delete_item(&conn, drill.id).unwrap();
        assert!(all_tags(&conn).unwrap().is_empty());
    }

    #[test]
    fn quantity_never_goes_negative() {
        let mut conn = test_db();
        let bulbs = item(&mut conn, "Bulbs", "", None, &[]);
        let after = adjust_quantity(&conn, bulbs.id, -10).unwrap();
        assert_eq!(after.quantity, 0);
    }

    #[test]
    fn nested_queries_reach_into_child_containers() {
        let mut conn = test_db();
        let garage = container(&conn, "Garage", "area", None);
        let shelf = container(&conn, "Shelf", "shelf", Some(garage.id));
        item(&mut conn, "Tent", "", Some(shelf.id), &[]);

        let direct = query_items(
            &conn,
            &ItemQuery {
                container_id: Some(garage.id),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            direct.is_empty(),
            "the tent is on the shelf, not loose in the garage"
        );

        let nested = query_items(
            &conn,
            &ItemQuery {
                container_id: Some(garage.id),
                include_nested: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(nested.len(), 1);
    }

    #[test]
    fn containers_holding_items_go_stale_and_checking_clears_it() {
        let mut conn = test_db();
        let bin = container(&conn, "Camping", "box", None);
        item(&mut conn, "Tent", "", Some(bin.id), &[]);
        // Backdate it beyond the default window.
        conn.execute(
            "UPDATE containers SET created_at = datetime('now', '-400 days') WHERE id = ?1",
            [bin.id],
        )
        .unwrap();

        let before = container_by_id(&conn, bin.id).unwrap();
        assert!(before.stale);
        assert_eq!(before.days_since_check, None, "never checked");
        assert!(before.age_days >= 399);

        let after = mark_checked(&conn, bin.id).unwrap();
        assert!(!after.stale);
        assert_eq!(after.days_since_check, Some(0));
        assert!(after.checked_at.is_some());
    }

    #[test]
    fn empty_containers_are_never_flagged_for_a_check() {
        let conn = test_db();
        let shelf = container(&conn, "Shelf", "shelf", None);
        conn.execute(
            "UPDATE containers SET created_at = datetime('now', '-900 days') WHERE id = ?1",
            [shelf.id],
        )
        .unwrap();
        assert!(
            !container_by_id(&conn, shelf.id).unwrap().stale,
            "nothing inside to verify"
        );
    }

    #[test]
    fn the_staleness_window_is_configurable() {
        let mut conn = test_db();
        let bin = container(&conn, "Camping", "box", None);
        item(&mut conn, "Tent", "", Some(bin.id), &[]);
        conn.execute(
            "UPDATE containers SET checked_at = datetime('now', '-60 days') WHERE id = ?1",
            [bin.id],
        )
        .unwrap();

        assert!(
            !container_by_id(&conn, bin.id).unwrap().stale,
            "60 days is inside the default"
        );
        crate::db::set_setting(&conn, "stale_after_days", "30").unwrap();
        assert!(
            container_by_id(&conn, bin.id).unwrap().stale,
            "but outside a 30 day window"
        );
    }

    #[test]
    fn a_shelf_reports_each_box_with_its_contents() {
        let mut conn = test_db();
        let shelf = container(&conn, "Shelf", "shelf", None);
        let camping = container(&conn, "Camping", "box", Some(shelf.id));
        let holiday = container(&conn, "Holiday", "bin", Some(shelf.id));
        item(&mut conn, "Tent", "", Some(camping.id), &[]);
        item(&mut conn, "Stove", "", Some(camping.id), &[]);
        item(&mut conn, "Wreath", "", Some(holiday.id), &[]);

        let detail = container_detail(&conn, shelf.id).unwrap();
        assert_eq!(detail.children.len(), 2);
        let names: Vec<Vec<String>> = detail
            .children
            .iter()
            .map(|c| c.items.iter().map(|i| i.name.clone()).collect())
            .collect();
        assert!(names.contains(&vec!["Stove".to_string(), "Tent".to_string()]));
        assert!(names.contains(&vec!["Wreath".to_string()]));
        assert_eq!(
            detail.nested_item_count, 3,
            "the shelf holds three things in total"
        );
    }

    #[test]
    fn renaming_a_tag_updates_every_item_using_it() {
        let mut conn = test_db();
        item(&mut conn, "Drill", "", None, &["tols"]);
        item(&mut conn, "Saw", "", None, &["tols"]);
        rename_tag(&mut conn, "tols", "tools").unwrap();

        let tags = all_tags(&conn).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "tools");
        assert_eq!(tags[0].item_count, 2);
    }

    #[test]
    fn renaming_onto_an_existing_tag_merges_them() {
        let mut conn = test_db();
        let drill = item(&mut conn, "Drill", "", None, &["power-tools"]);
        item(&mut conn, "Saw", "", None, &["tools"]);
        rename_tag(&mut conn, "power-tools", "tools").unwrap();

        let tags = all_tags(&conn).unwrap();
        assert_eq!(tags.len(), 1, "the two tags become one");
        assert_eq!(tags[0].item_count, 2);
        assert_eq!(
            item_by_id(&conn, drill.id).unwrap().tags,
            vec!["tools".to_string()]
        );
    }

    #[test]
    fn deleting_a_tag_leaves_the_items_alone() {
        let mut conn = test_db();
        let drill = item(&mut conn, "Drill", "", None, &["tools", "loud"]);
        delete_tag(&conn, "loud").unwrap();
        assert_eq!(
            item_by_id(&conn, drill.id).unwrap().tags,
            vec!["tools".to_string()]
        );
    }

    fn with_barcode(
        conn: &mut Connection,
        name: &str,
        barcode: &str,
        container: Option<i64>,
    ) -> Item {
        create_item(
            conn,
            &ItemInput {
                name: name.into(),
                description: String::new(),
                quantity: 1,
                container_id: container,
                photo_id: None,
                tags: vec![],
                barcode: Some(barcode.into()),
            },
        )
        .unwrap()
    }

    #[test]
    fn scanning_a_label_code_finds_the_container() {
        let mut conn = test_db();
        let bin = container(&conn, "Camping", "box", None);
        item(&mut conn, "Tent", "", Some(bin.id), &[]);

        match resolve_scan(&conn, &bin.code.to_lowercase()).unwrap() {
            ScanResult::Container { container } => {
                assert_eq!(container.container.id, bin.id);
                assert_eq!(container.items.len(), 1, "contents come back with the box");
            }
            other => panic!("expected a container, got {other:?}"),
        }
    }

    #[test]
    fn scanning_a_product_barcode_finds_the_item() {
        let mut conn = test_db();
        let drill = with_barcode(&mut conn, "Drill", "012345678905", None);
        match resolve_scan(&conn, " 012345678905 ").unwrap() {
            ScanResult::Item { item } => assert_eq!(item.id, drill.id),
            other => panic!("expected an item, got {other:?}"),
        }
    }

    #[test]
    fn scanning_a_barcode_stuck_on_a_box_finds_the_box() {
        let conn = test_db();
        let bin = create_container(
            &conn,
            &ContainerInput {
                name: "Camping".into(),
                kind: "box".into(),
                parent_id: None,
                notes: String::new(),
                photo_id: None,
                code: None,
                barcode: Some("9001234567890".into()),
            },
        )
        .unwrap();
        match resolve_scan(&conn, "9001234567890").unwrap() {
            ScanResult::Container { container } => assert_eq!(container.container.id, bin.id),
            other => panic!("expected a container, got {other:?}"),
        }
    }

    #[test]
    fn an_unrecognised_scan_reports_the_raw_code() {
        let conn = test_db();
        match resolve_scan(&conn, "5060337502900").unwrap() {
            ScanResult::Unknown { code } => assert_eq!(code, "5060337502900"),
            other => panic!("expected unknown, got {other:?}"),
        }
    }

    #[test]
    fn a_barcode_cannot_be_assigned_to_two_things() {
        let mut conn = test_db();
        with_barcode(&mut conn, "Drill", "012345678905", None);
        let clash = create_item(
            &mut conn,
            &ItemInput {
                name: "Another drill".into(),
                description: String::new(),
                quantity: 1,
                container_id: None,
                photo_id: None,
                tags: vec![],
                barcode: Some("012345678905".into()),
            },
        );
        assert!(clash.is_err(), "one scan must resolve to one thing");
        assert!(clash
            .unwrap_err()
            .message
            .contains("already assigned to Drill"));
    }

    #[test]
    fn blank_barcodes_are_stored_as_none_and_never_clash() {
        let mut conn = test_db();
        let a = with_barcode(&mut conn, "Hammer", "   ", None);
        let b = with_barcode(&mut conn, "Saw", "", None);
        assert_eq!(a.barcode, None);
        assert_eq!(b.barcode, None);
    }

    #[test]
    fn ages_are_reported_in_seconds_not_whole_days() {
        let conn = test_db();
        let bin = container(&conn, "Camping", "box", None);
        conn.execute(
            "UPDATE containers SET created_at = datetime('now', '-40 minutes') WHERE id = ?1",
            [bin.id],
        )
        .unwrap();
        let loaded = container_by_id(&conn, bin.id).unwrap();
        assert_eq!(loaded.age_days, 0, "less than a day old");
        // Whole days alone cannot tell 40 minutes from 20 hours.
        assert!(
            (2350..2450).contains(&loaded.age_seconds),
            "expected about 2400 seconds, got {}",
            loaded.age_seconds
        );
    }

    #[test]
    fn a_clock_that_moves_backwards_does_not_refresh_anything() {
        let mut conn = test_db();
        let bin = container(&conn, "Camping", "box", None);
        item(&mut conn, "Tent", "", Some(bin.id), &[]);
        conn.execute(
            "UPDATE containers SET checked_at = datetime('now', '-300 days') WHERE id = ?1",
            [bin.id],
        )
        .unwrap();
        assert!(container_by_id(&conn, bin.id).unwrap().stale);

        // The machine has seen time run a year past the current clock reading,
        // as it would after a clock is wound back or a dead RTC resets.
        crate::db::set_setting(&conn, "clock_high_water", "2099-01-01 00:00:00").unwrap();

        let after = container_by_id(&conn, bin.id).unwrap();
        assert!(
            after.stale,
            "winding the clock back must not clear a check-up"
        );
        assert!(
            after.age_days > 300,
            "age is measured from the latest time seen, not the earlier clock"
        );

        let status = clock_status(&conn).unwrap();
        assert!(
            status.behind_seconds.unwrap() > 0,
            "and the discrepancy is reported"
        );
    }

    #[test]
    fn a_normal_clock_reports_no_discrepancy() {
        let conn = test_db();
        touch_clock(&conn).unwrap();
        assert_eq!(clock_status(&conn).unwrap().behind_seconds, None);
    }

    #[test]
    fn items_must_be_named() {
        let mut conn = test_db();
        let blank = create_item(
            &mut conn,
            &ItemInput {
                name: "   ".into(),
                description: String::new(),
                quantity: 1,
                container_id: None,
                photo_id: None,
                tags: vec![],
                barcode: None,
            },
        );
        assert!(blank.is_err());
    }
}
