//! Photos, QR codes and printable labels — the "know what's in the box
//! without opening it" half of the app.

use crate::db;
use crate::error::{AppError, AppResult};
use crate::store;
use crate::AppState;
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, Response};
use axum::Json;
use qrcode::render::svg;
use qrcode::QrCode;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;

const MAX_PHOTO_BYTES: usize = 12 * 1024 * 1024;

/// Trusts file contents, not the client's Content-Type header.
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub async fn upload_photo(
    State(st): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad_request(format!("malformed upload: {e}")))?
    {
        if field.name() != Some("file") && field.name() != Some("photo") {
            continue;
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::bad_request(format!("could not read upload: {e}")))?;
        if data.is_empty() {
            return Err(AppError::bad_request("uploaded file is empty"));
        }
        if data.len() > MAX_PHOTO_BYTES {
            return Err(AppError::bad_request("image is larger than 12 MB"));
        }
        let mime = sniff_image_mime(&data)
            .ok_or_else(|| AppError::bad_request("only JPEG, PNG, GIF or WebP images are accepted"))?;
        let bytes = data.to_vec();
        let id = db::run(&st.pool, move |c| {
            c.execute(
                "INSERT INTO photos (mime, bytes) VALUES (?1, ?2)",
                rusqlite::params![mime, bytes],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await?;
        return Ok(Json(json!({ "id": id, "url": format!("/photos/{id}") })));
    }
    Err(AppError::bad_request("no file field in upload"))
}

pub async fn get_photo(State(st): State<AppState>, Path(id): Path<i64>) -> AppResult<Response> {
    let (mime, bytes): (String, Vec<u8>) = db::run(&st.pool, move |c| {
        Ok(c.query_row("SELECT mime, bytes FROM photos WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?)
    })
    .await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        // Photo bytes never change once stored, so they can be cached hard.
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(Body::from(bytes))
        .map_err(|e| AppError::internal(e.to_string()))
}

/// Drops photos no longer attached to anything. Called after deletions.
pub fn prune_photos(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "DELETE FROM photos
          WHERE id NOT IN (SELECT photo_id FROM items WHERE photo_id IS NOT NULL)
            AND id NOT IN (SELECT photo_id FROM containers WHERE photo_id IS NOT NULL)",
        [],
    )?;
    Ok(())
}

// ----------------------------------------------------------------- QR codes

fn qr_svg(data: &str, size: u32) -> AppResult<String> {
    let code = QrCode::new(data.as_bytes())
        .map_err(|e| AppError::internal(format!("could not build QR code: {e}")))?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(size, size)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

#[derive(Debug, Deserialize)]
pub struct QrParams {
    pub size: Option<u32>,
}

pub async fn container_qr(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<QrParams>,
) -> AppResult<Response> {
    let base = st.public_url();
    let code = db::run(&st.pool, move |c| store::container_by_id(c, id).map(|x| x.code)).await?;
    let svg = qr_svg(&format!("{base}/b/{code}"), params.size.unwrap_or(240).clamp(64, 2048))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/svg+xml")
        .body(Body::from(svg))
        .map_err(|e| AppError::internal(e.to_string()))
}

// ------------------------------------------------------------------- labels

#[derive(Debug, Deserialize)]
pub struct LabelParams {
    /// Comma-separated label codes, e.g. `BX-7K3Q,BN-2M9F`.
    pub codes: Option<String>,
    /// Print labels for every container.
    #[serde(default)]
    pub all: bool,
    /// `large` (with contents list) or `small` (grid of QR + name).
    pub size: Option<String>,
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Renders a printable sheet of labels. Each label carries a QR code that
/// opens that container's page, plus a preview of what's inside so the label
/// is still useful to someone without a phone.
pub async fn print_labels(
    State(st): State<AppState>,
    Query(params): Query<LabelParams>,
) -> AppResult<Html<String>> {
    let base = st.public_url();
    let large = params.size.as_deref() != Some("small");

    let wanted: Option<Vec<String>> = params.codes.as_ref().map(|c| {
        c.split(',').map(store::normalize_code).filter(|s| !s.is_empty()).collect()
    });

    let containers = db::run(&st.pool, {
        let wanted = wanted.clone();
        move |c| {
            let all = store::all_containers(c)?;
            let selected: Vec<_> = match &wanted {
                Some(codes) if !codes.is_empty() => all
                    .into_iter()
                    .filter(|x| codes.contains(&x.code.to_uppercase()))
                    .collect(),
                _ => all,
            };
            let mut out = Vec::new();
            for container in selected {
                let items = store::query_items(
                    c,
                    &store::ItemQuery {
                        container_id: Some(container.id),
                        limit: Some(14),
                        ..Default::default()
                    },
                )?;
                out.push((container, items));
            }
            Ok(out)
        }
    })
    .await?;

    if containers.is_empty() && (params.all || wanted.is_some()) {
        return Ok(Html(page_shell("No labels", "<p class=\"empty\">No containers matched. Add a box first, then print its label.</p>".into())));
    }

    let mut body = String::new();
    body.push_str(
        r#"<div class="toolbar no-print">
             <button onclick="window.print()">Print these labels</button>
             <a href="/#/labels">Back to the app</a>
             <span class="hint">Tip: print at 100% scale, then tape one label per box.</span>
           </div>"#,
    );
    body.push_str(if large { "<div class=\"sheet large\">" } else { "<div class=\"sheet small\">" });

    for (container, items) in &containers {
        let url = format!("{}/b/{}", base, container.code);
        let qr = qr_svg(&url, if large { 260 } else { 180 })?;
        let contents = if large && !items.is_empty() {
            let lis: String = items
                .iter()
                .map(|i| {
                    let qty = if i.quantity > 1 {
                        format!(" <span class=\"qty\">×{}</span>", i.quantity)
                    } else {
                        String::new()
                    };
                    format!("<li>{}{}</li>", escape_html(&i.name), qty)
                })
                .collect();
            let more = if container.item_count > items.len() as i64 {
                format!(
                    "<li class=\"more\">+ {} more — scan for the full list</li>",
                    container.item_count - items.len() as i64
                )
            } else {
                String::new()
            };
            format!("<ul class=\"contents\">{lis}{more}</ul>")
        } else {
            String::new()
        };
        let location = if container.path.contains(" / ") {
            let parent =
                container.path.rsplit_once(" / ").map(|(head, _)| head.to_string()).unwrap_or_default();
            format!("<div class=\"where\">{}</div>", escape_html(&parent))
        } else {
            String::new()
        };
        body.push_str(&format!(
            r#"<div class="label">
                 <div class="qr">{qr}</div>
                 <div class="meta">
                   <div class="code">{code}</div>
                   <div class="name">{name}</div>
                   {location}
                   {contents}
                 </div>
               </div>"#,
            qr = qr,
            code = escape_html(&container.code),
            name = escape_html(&container.name),
            location = location,
            contents = contents,
        ));
    }
    body.push_str("</div>");
    Ok(Html(page_shell("Labels", body)))
}

fn page_shell(title: &str, body: String) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · Garage Inventory</title>
<style>
  * {{ box-sizing: border-box; }}
  body {{ font: 15px/1.45 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
         margin: 0; padding: 16px; color: #111; background: #f6f7f9; }}
  .toolbar {{ display: flex; gap: 14px; align-items: center; margin-bottom: 18px; flex-wrap: wrap; }}
  .toolbar button {{ font: inherit; font-weight: 600; padding: 9px 16px; border-radius: 8px;
                     border: 0; background: #2f6fed; color: #fff; cursor: pointer; }}
  .toolbar a {{ color: #2f6fed; }}
  .hint {{ color: #666; font-size: 13px; }}
  .empty {{ color: #666; }}
  .sheet {{ display: grid; gap: 10px; }}
  .sheet.large {{ grid-template-columns: repeat(2, 1fr); }}
  .sheet.small {{ grid-template-columns: repeat(3, 1fr); }}
  .label {{ display: flex; gap: 12px; padding: 12px; border: 2px dashed #bbb; border-radius: 10px;
            background: #fff; break-inside: avoid; page-break-inside: avoid; }}
  .label .qr {{ flex: 0 0 auto; }}
  .label .qr svg {{ display: block; width: 118px; height: 118px; }}
  .sheet.small .label .qr svg {{ width: 86px; height: 86px; }}
  .meta {{ min-width: 0; }}
  .code {{ font: 700 15px ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .5px; }}
  .name {{ font-weight: 700; font-size: 17px; margin-top: 2px; word-break: break-word; }}
  .sheet.small .name {{ font-size: 14px; }}
  .where {{ color: #666; font-size: 12px; margin-top: 2px; }}
  .contents {{ margin: 8px 0 0; padding-left: 16px; font-size: 12px; color: #333; columns: 1; }}
  .contents li {{ margin: 1px 0; }}
  .contents .qty {{ color: #666; }}
  .contents .more {{ color: #888; font-style: italic; list-style: none; margin-left: -16px; }}
  @media print {{
    body {{ background: #fff; padding: 0; }}
    .no-print {{ display: none !important; }}
    .label {{ border-color: #ccc; }}
    @page {{ margin: 10mm; }}
  }}
</style>
</head><body>{body}</body></html>"#
    )
}
