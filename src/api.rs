use crate::db;
use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::store::{self, ItemQuery};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Default)]
pub struct ItemQueryParams {
    pub q: Option<String>,
    pub container: Option<i64>,
    #[serde(default)]
    pub nested: bool,
    pub tag: Option<String>,
    #[serde(default)]
    pub unfiled: bool,
    pub sort: Option<String>,
    pub limit: Option<i64>,
}

impl From<ItemQueryParams> for ItemQuery {
    fn from(p: ItemQueryParams) -> Self {
        ItemQuery {
            q: p.q.filter(|s| !s.trim().is_empty()),
            container_id: p.container,
            container_ids: None,
            include_nested: p.nested,
            tag: p.tag.filter(|s| !s.trim().is_empty()),
            unfiled: p.unfiled,
            sort: p.sort,
            limit: p.limit,
        }
    }
}

// ---------------------------------------------------------------- containers

pub async fn list_containers(State(st): State<AppState>) -> AppResult<Json<Vec<Container>>> {
    db::run(&st.pool, |c| store::all_containers(c))
        .await
        .map(Json)
}

pub async fn get_container(
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ContainerDetail>> {
    db::run(&st.pool, move |c| store::container_detail(c, id))
        .await
        .map(Json)
}

pub async fn get_container_by_code(
    State(st): State<AppState>,
    Path(code): Path<String>,
) -> AppResult<Json<ContainerDetail>> {
    db::run(&st.pool, move |c| {
        let container = store::container_by_code(c, &code)?;
        store::container_detail(c, container.id)
    })
    .await
    .map(Json)
}

pub async fn create_container(
    State(st): State<AppState>,
    Json(input): Json<ContainerInput>,
) -> AppResult<Json<Container>> {
    db::run(&st.pool, move |c| store::create_container(c, &input))
        .await
        .map(Json)
}

pub async fn update_container(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ContainerInput>,
) -> AppResult<Json<Container>> {
    db::run(&st.pool, move |c| store::update_container(c, id, &input))
        .await
        .map(Json)
}

pub async fn delete_container(
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    db::run(&st.pool, move |c| {
        store::delete_container(c, id)?;
        crate::media::prune_photos(c)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Marks a container as just-verified: someone looked inside and the listing
/// matches reality.
pub async fn verify_container(
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Container>> {
    db::run(&st.pool, move |c| store::mark_checked(c, id))
        .await
        .map(Json)
}

/// Containers holding items that haven't been verified within the staleness
/// window, most overdue first.
pub async fn stale_containers(State(st): State<AppState>) -> AppResult<Json<Value>> {
    db::run(&st.pool, |c| {
        let days = store::stale_after_days(c);
        let mut stale: Vec<Container> = store::all_containers(c)?
            .into_iter()
            .filter(|x| x.stale)
            .collect();
        stale.sort_by_key(|c| std::cmp::Reverse(c.age_days));
        Ok(Json(
            json!({ "stale_after_days": days, "containers": stale }),
        ))
    })
    .await
}

// --------------------------------------------------------------------- items

pub async fn list_items(
    State(st): State<AppState>,
    Query(params): Query<ItemQueryParams>,
) -> AppResult<Json<Vec<Item>>> {
    let query: ItemQuery = params.into();
    db::run(&st.pool, move |c| store::query_items(c, &query))
        .await
        .map(Json)
}

pub async fn get_item(State(st): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<Item>> {
    db::run(&st.pool, move |c| store::item_by_id(c, id))
        .await
        .map(Json)
}

pub async fn create_item(
    State(st): State<AppState>,
    Json(input): Json<ItemInput>,
) -> AppResult<Json<Item>> {
    db::run(&st.pool, move |c| store::create_item(c, &input))
        .await
        .map(Json)
}

pub async fn update_item(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ItemInput>,
) -> AppResult<Json<Item>> {
    db::run(&st.pool, move |c| store::update_item(c, id, &input))
        .await
        .map(Json)
}

pub async fn delete_item(
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    db::run(&st.pool, move |c| {
        store::delete_item(c, id)?;
        crate::media::prune_photos(c)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

pub async fn move_item(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<MoveInput>,
) -> AppResult<Json<Item>> {
    db::run(&st.pool, move |c| {
        store::move_item(c, id, input.container_id)
    })
    .await
    .map(Json)
}

pub async fn adjust_quantity(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<QuantityInput>,
) -> AppResult<Json<Item>> {
    db::run(&st.pool, move |c| {
        store::adjust_quantity(c, id, input.delta)
    })
    .await
    .map(Json)
}

/// Bulk move — "I just emptied this box into that one".
#[derive(Debug, Deserialize)]
pub struct BulkMoveInput {
    pub item_ids: Vec<i64>,
    pub container_id: Option<i64>,
}

pub async fn bulk_move(
    State(st): State<AppState>,
    Json(input): Json<BulkMoveInput>,
) -> AppResult<Json<Value>> {
    db::run(&st.pool, move |c| {
        let tx = c.transaction()?;
        for id in &input.item_ids {
            tx.execute(
                "UPDATE items SET container_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![input.container_id, id],
            )?;
        }
        tx.commit()?;
        Ok(Json(json!({ "ok": true, "moved": input.item_ids.len() })))
    })
    .await
}

/// Resolves a scanned barcode or label code to whatever it identifies. This is
/// the endpoint a barcode scanner drives: everything else follows from it.
pub async fn scan(
    State(st): State<AppState>,
    Path(code): Path<String>,
) -> AppResult<Json<ScanResult>> {
    db::run(&st.pool, move |c| store::resolve_scan(c, &code))
        .await
        .map(Json)
}

// ------------------------------------------------------------ search & meta

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub limit: Option<i64>,
}

/// Searches items *and* containers in one shot: typing "camping" should find
/// both the tent and the box called "Camping gear".
pub async fn search(
    State(st): State<AppState>,
    Query(params): Query<SearchParams>,
) -> AppResult<Json<Value>> {
    let q = params.q.unwrap_or_default().trim().to_string();
    let limit = params.limit.unwrap_or(200);
    db::run(&st.pool, move |c| {
        if q.is_empty() {
            return Ok(Json(json!({ "query": q, "items": [], "containers": [] })));
        }
        let items = store::query_items(
            c,
            &ItemQuery {
                q: Some(q.clone()),
                limit: Some(limit),
                ..Default::default()
            },
        )?;
        let terms: Vec<String> = q
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .take(8)
            .collect();
        let containers: Vec<Container> = store::all_containers(c)?
            .into_iter()
            .filter(|ct| {
                let haystack =
                    format!("{} {} {} {}", ct.name, ct.code, ct.notes, ct.path).to_lowercase();
                terms.iter().all(|t| haystack.contains(t))
            })
            .take(50)
            .collect();
        Ok(Json(
            json!({ "query": q, "items": items, "containers": containers }),
        ))
    })
    .await
}

pub async fn list_tags(State(st): State<AppState>) -> AppResult<Json<Vec<TagCount>>> {
    db::run(&st.pool, |c| store::all_tags(c)).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct TagRename {
    pub name: String,
}

pub async fn rename_tag(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<TagRename>,
) -> AppResult<Json<Value>> {
    db::run(&st.pool, move |c| {
        let renamed = store::rename_tag(c, &name, &body.name)?;
        Ok(Json(json!({ "ok": true, "name": renamed })))
    })
    .await
}

pub async fn delete_tag(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<Value>> {
    db::run(&st.pool, move |c| {
        store::delete_tag(c, &name)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Liveness probe: cheap, and it touches the database so a wedged file shows
/// up as unhealthy rather than merely quiet.
pub async fn health(State(st): State<AppState>) -> AppResult<Json<Value>> {
    db::run(&st.pool, |c| {
        let containers: i64 = c.query_row("SELECT COUNT(*) FROM containers", [], |r| r.get(0))?;
        Ok(Json(json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "containers": containers,
        })))
    })
    .await
}

pub async fn stats(State(st): State<AppState>) -> AppResult<Json<Stats>> {
    db::run(&st.pool, |c| store::stats(c)).await.map(Json)
}

/// Everything the frontend needs on first paint, in one round trip.
pub async fn bootstrap(State(st): State<AppState>) -> AppResult<Json<Value>> {
    let base_url = st.public_url();
    db::run(&st.pool, move |c| {
        Ok(Json(json!({
            "containers": store::all_containers(c)?,
            "tags": store::all_tags(c)?,
            "stats": store::stats(c)?,
            "kinds": store::KINDS,
            "public_url": base_url,
            "stale_after_days": store::stale_after_days(c),
        })))
    })
    .await
}

pub async fn get_settings(State(st): State<AppState>) -> AppResult<Json<Value>> {
    let effective = st.public_url();
    let detected = st.detected_url();
    db::run(&st.pool, move |c| {
        let stored = db::get_setting(c, "public_url")
            .map_err(|e| AppError::internal(e.to_string()))?
            .unwrap_or_default();
        Ok(Json(json!({
            "public_url": stored,
            "effective_public_url": effective,
            "detected_url": detected,
            "stale_after_days": store::stale_after_days(c),
        })))
    })
    .await
}

pub async fn update_settings(
    State(st): State<AppState>,
    Json(body): Json<HashMap<String, String>>,
) -> AppResult<Json<Value>> {
    if let Some(raw) = body.get("stale_after_days") {
        let days: i64 = raw
            .trim()
            .parse()
            .map_err(|_| AppError::bad_request("re-check reminder must be a number of days"))?;
        if !(1..=3650).contains(&days) {
            return Err(AppError::bad_request(
                "re-check reminder must be between 1 and 3650 days",
            ));
        }
        db::run(&st.pool, move |c| {
            db::set_setting(c, "stale_after_days", &days.to_string())
                .map_err(|e| AppError::internal(e.to_string()))
        })
        .await?;
    }
    if !body.contains_key("public_url") {
        return Ok(Json(
            json!({ "ok": true, "effective_public_url": st.public_url() }),
        ));
    }
    let url = body
        .get("public_url")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let has_scheme = url.starts_with("http://") || url.starts_with("https://");
    if !url.is_empty() && !has_scheme {
        return Err(AppError::bad_request(
            "public URL must start with http:// or https://",
        ));
    }
    let url = url.trim_end_matches('/').to_string();
    let stored = url.clone();
    db::run(&st.pool, move |c| {
        db::set_setting(c, "public_url", &stored).map_err(|e| AppError::internal(e.to_string()))?;
        Ok(())
    })
    .await?;
    *st.public_url_override.write().unwrap() = if url.is_empty() { None } else { Some(url) };
    Ok(Json(
        json!({ "ok": true, "effective_public_url": st.public_url() }),
    ))
}
