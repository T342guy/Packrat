//! Export and import. The whole inventory is a single SQLite file, but a
//! plain-JSON export means the data outlives this program.

use crate::db;
use crate::error::{AppError, AppResult};
use crate::store;
use crate::AppState;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn b64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut table = [255u8; 256];
    for (i, c) in B64.iter().enumerate() {
        table[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in input.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = table[c as usize];
        if v == 255 {
            return Err(format!("invalid base64 character '{}'", c as char));
        }
        buffer = (buffer << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct ExportParams {
    /// Include photo bytes (base64). Off by default — exports stay small.
    #[serde(default)]
    pub photos: bool,
}

pub async fn export_json(
    State(st): State<AppState>,
    Query(params): Query<ExportParams>,
) -> AppResult<Response> {
    let payload = db::run(&st.pool, move |c| {
        let containers = store::all_containers(c)?;
        let items = store::query_items(c, &store::ItemQuery::default())?;
        let photos: Vec<Value> = if params.photos {
            let mut stmt = c.prepare("SELECT id, mime, bytes FROM photos ORDER BY id")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
            })?;
            rows.map(|row| {
                row.map(|(id, mime, bytes)| {
                    json!({ "id": id, "mime": mime, "data": b64_encode(&bytes) })
                })
            })
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Ok(json!({
            "format": "packrat",
            "version": 1,
            "exported_at": now(c),
            "includes_photos": params.photos,
            "containers": containers,
            "items": items,
            "photos": photos,
        }))
    })
    .await?;

    let body = serde_json::to_vec_pretty(&payload)
        .map_err(|e| AppError::internal(format!("could not serialise export: {e}")))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"packrat-export.json\"",
        )
        .body(axum::body::Body::from(body))
        .map_err(|e| AppError::internal(e.to_string()))
}

fn now(conn: &rusqlite::Connection) -> String {
    conn.query_row("SELECT datetime('now')", [], |r| r.get::<_, String>(0)).unwrap_or_default()
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub async fn export_csv(State(st): State<AppState>) -> AppResult<Response> {
    let csv = db::run(&st.pool, |c| {
        let items = store::query_items(c, &store::ItemQuery::default())?;
        let mut out = String::from(
            "name,quantity,description,tags,container_code,container_name,location,created_at,updated_at\n",
        );
        for i in items {
            let row = [
                i.name,
                i.quantity.to_string(),
                i.description,
                i.tags.join(" "),
                i.container_code.unwrap_or_default(),
                i.container_name.unwrap_or_default(),
                i.container_path.unwrap_or_default(),
                i.created_at,
                i.updated_at,
            ]
            .iter()
            .map(|f| csv_field(f))
            .collect::<Vec<_>>()
            .join(",");
            out.push_str(&row);
            out.push('\n');
        }
        Ok(out)
    })
    .await?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"packrat.csv\"")
        .body(axum::body::Body::from(csv))
        .map_err(|e| AppError::internal(e.to_string()))
}

#[derive(Debug, Deserialize)]
struct BackupContainer {
    id: i64,
    code: String,
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    parent_id: Option<i64>,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    photo_id: Option<i64>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BackupItem {
    id: i64,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "one")]
    quantity: i64,
    #[serde(default)]
    container_id: Option<i64>,
    #[serde(default)]
    photo_id: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Deserialize)]
struct BackupPhoto {
    id: i64,
    mime: String,
    data: String,
}

#[derive(Debug, Deserialize)]
pub struct Backup {
    #[serde(default)]
    containers: Vec<BackupContainer>,
    #[serde(default)]
    items: Vec<BackupItem>,
    #[serde(default)]
    photos: Vec<BackupPhoto>,
}

#[derive(Debug, Deserialize)]
pub struct ImportParams {
    /// Must be `replace` — importing wipes the current inventory.
    pub confirm: Option<String>,
}

/// Restores an export, replacing everything currently stored. Ids are kept so
/// printed labels and photo references still line up after a restore.
pub async fn import_json(
    State(st): State<AppState>,
    Query(params): Query<ImportParams>,
    Json(backup): Json<Backup>,
) -> AppResult<Json<Value>> {
    if params.confirm.as_deref() != Some("replace") {
        return Err(AppError::bad_request(
            "importing replaces the whole inventory; call again with ?confirm=replace",
        ));
    }
    db::run(&st.pool, move |c| {
        let tx = c.transaction()?;
        // Rows arrive in arbitrary order, so parent links can point forward.
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        tx.execute_batch(
            "DELETE FROM item_tags; DELETE FROM tags; DELETE FROM items;
             DELETE FROM containers; DELETE FROM photos;",
        )?;

        for p in &backup.photos {
            let bytes = b64_decode(&p.data)
                .map_err(|e| AppError::bad_request(format!("photo {}: {e}", p.id)))?;
            tx.execute(
                "INSERT INTO photos (id, mime, bytes) VALUES (?1, ?2, ?3)",
                rusqlite::params![p.id, p.mime, bytes],
            )?;
        }
        for ct in &backup.containers {
            let kind = store::normalize_kind(&ct.kind);
            tx.execute(
                "INSERT INTO containers (id, code, name, kind, parent_id, notes, photo_id,
                                         created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                         COALESCE(?8, datetime('now')), COALESCE(?9, datetime('now')))",
                rusqlite::params![
                    ct.id,
                    store::normalize_code(&ct.code),
                    ct.name.trim(),
                    kind,
                    ct.parent_id,
                    ct.notes,
                    ct.photo_id,
                    ct.created_at,
                    ct.updated_at
                ],
            )?;
        }
        for it in &backup.items {
            tx.execute(
                "INSERT INTO items (id, name, description, quantity, container_id, photo_id,
                                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                         COALESCE(?7, datetime('now')), COALESCE(?8, datetime('now')))",
                rusqlite::params![
                    it.id,
                    it.name.trim(),
                    it.description,
                    it.quantity.max(0),
                    it.container_id,
                    it.photo_id,
                    it.created_at,
                    it.updated_at
                ],
            )?;
            for tag in &it.tags {
                let tag = tag.trim();
                if tag.is_empty() {
                    continue;
                }
                tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
                let tag_id: i64 = tx.query_row(
                    "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                    [tag],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
                    rusqlite::params![it.id, tag_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(Json(json!({
            "ok": true,
            "containers": backup.containers.len(),
            "items": backup.items.len(),
            "photos": backup.photos.len(),
        })))
    })
    .await
}
