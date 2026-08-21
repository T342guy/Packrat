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

/// The barcode printed on a container's label: a pre-printed one if it has
/// been assigned, otherwise the container's own label code.
fn scannable_code(container: &crate::models::Container) -> String {
    container.barcode.clone().unwrap_or_else(|| container.code.clone())
}

pub async fn container_barcode(
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let container = db::run(&st.pool, move |c| store::container_by_id(c, id)).await?;
    let svg = crate::barcode::code128_svg(&scannable_code(&container), 30)
        .map_err(AppError::bad_request)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/svg+xml")
        .body(Body::from(svg))
        .map_err(|e| AppError::internal(e.to_string()))
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

/// A physical label size. Roll formats (`page_mm`) print one label per page,
/// which is how a DYMO LabelWriter expects to be driven from a browser; sheet
/// formats tile labels onto ordinary A4/Letter paper.
pub struct LabelFormat {
    pub id: &'static str,
    pub name: &'static str,
    /// Page size in millimetres for roll/label-printer stock.
    pub page_mm: Option<(f32, f32)>,
    pub columns: usize,
    pub qr_mm: f32,
    pub show_name: bool,
    pub show_location: bool,
    pub show_contents: bool,
    pub max_items: usize,
    /// Square labels stack the QR above the code instead of beside it.
    pub stacked: bool,
}

/// Sizes are the printable label dimensions in millimetres, long edge first
/// for the roll formats — that is the orientation a LabelWriter prints in.
pub const LABEL_FORMATS: &[LabelFormat] = &[
    LabelFormat {
        id: "sheet-large",
        name: "A4/Letter sheet — with contents",
        page_mm: None,
        columns: 2,
        qr_mm: 31.0,
        show_name: true,
        show_location: true,
        show_contents: true,
        max_items: 14,
        stacked: false,
    },
    LabelFormat {
        id: "sheet-small",
        name: "A4/Letter sheet — compact",
        page_mm: None,
        columns: 3,
        qr_mm: 23.0,
        show_name: true,
        show_location: true,
        show_contents: false,
        max_items: 0,
        stacked: false,
    },
    LabelFormat {
        id: "dymo-30332",
        name: "DYMO 30332 — 1\" × 1\" square",
        page_mm: Some((25.4, 25.4)),
        columns: 1,
        qr_mm: 16.0,
        show_name: true,
        show_location: false,
        show_contents: false,
        max_items: 0,
        stacked: true,
    },
    LabelFormat {
        id: "dymo-30336",
        name: "DYMO 30336 — 1\" × 2⅛\" multipurpose",
        page_mm: Some((54.0, 25.4)),
        columns: 1,
        qr_mm: 21.0,
        show_name: true,
        show_location: false,
        show_contents: false,
        max_items: 0,
        stacked: false,
    },
    LabelFormat {
        id: "dymo-30334",
        name: "DYMO 30334 — 2¼\" × 1¼\" multipurpose",
        page_mm: Some((57.15, 31.75)),
        columns: 1,
        qr_mm: 26.0,
        show_name: true,
        show_location: true,
        show_contents: true,
        max_items: 4,
        stacked: false,
    },
    LabelFormat {
        id: "dymo-30252",
        name: "DYMO 30252 — 1⅛\" × 3½\" address",
        page_mm: Some((88.9, 28.6)),
        columns: 1,
        qr_mm: 24.0,
        show_name: true,
        show_location: true,
        show_contents: true,
        max_items: 5,
        stacked: false,
    },
    LabelFormat {
        id: "dymo-30323",
        name: "DYMO 30323 — 2⅛\" × 4\" shipping",
        page_mm: Some((101.6, 54.0)),
        columns: 1,
        qr_mm: 44.0,
        show_name: true,
        show_location: true,
        show_contents: true,
        max_items: 12,
        stacked: false,
    },
];

fn find_format(id: &str) -> Option<&'static LabelFormat> {
    LABEL_FORMATS.iter().find(|f| f.id == id)
}

/// Builds a format for arbitrary label stock given its size in millimetres,
/// so label printers other than the presets above still work.
fn custom_format(width: f32, height: f32) -> LabelFormat {
    let width = width.clamp(15.0, 300.0);
    let height = height.clamp(15.0, 300.0);
    let short = width.min(height);
    let area = width * height;
    LabelFormat {
        id: "custom",
        name: "Custom size",
        page_mm: Some((width, height)),
        columns: 1,
        qr_mm: (short * 0.62).clamp(12.0, 60.0),
        show_name: short >= 20.0,
        show_location: area >= 1600.0,
        show_contents: area >= 2200.0,
        max_items: if area >= 4000.0 { 10 } else { 4 },
        stacked: width < height * 1.35 && width > height * 0.75,
    }
}

#[derive(Debug, Deserialize)]
pub struct LabelParams {
    /// Comma-separated label codes, e.g. `BX-7K3Q,BN-2M9F`.
    pub codes: Option<String>,
    /// Print labels for every container.
    #[serde(default)]
    pub all: bool,
    /// Label stock: one of `LABEL_FORMATS`, or `custom` with `w`/`h`.
    pub format: Option<String>,
    /// Legacy alias kept working: `large` / `small` sheet layouts.
    pub size: Option<String>,
    /// Custom label width/height in millimetres.
    pub w: Option<f32>,
    pub h: Option<f32>,
    /// `auto` (default), `qr`, `barcode` or `both`.
    pub symbols: Option<String>,
    /// Cut-and-tape margins on paper sheets: `on` (default) or `off`.
    pub tape: Option<String>,
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

// Label geometry, in millimetres.
const PAD_MM: f32 = 1.6;
const GAP_MM: f32 = 1.6;
const BAR_HEIGHT_MM: f32 = 6.5;
/// Narrower bars than this and a 1D laser scanner starts failing.
const MIN_BAR_MM: f32 = 0.33;
/// Smaller QR modules than this and phone cameras start struggling.
const MIN_QR_MODULE_MM: f32 = 0.40;
/// Usable width of a portrait A4/Letter sheet inside its print margins.
const SHEET_WIDTH_MM: f32 = 190.0;

#[derive(PartialEq, Clone, Copy)]
enum BarcodePlace {
    /// Full width beneath the QR and text — the most bar width available.
    Below,
    /// Beside the QR, under the text: better when the label is wide and short,
    /// because the QR keeps its full height.
    Beside,
}

/// Worked-out sizes for one print job. Both symbols compete for the same
/// millimetres, so this decides once and reports what it produced.
struct Layout {
    qr_mm: f32,
    qr_module_mm: f32,
    bar_mm: f32,
    place: BarcodePlace,
    max_items: usize,
    show_location: bool,
}

fn plan_layout(
    format: &LabelFormat,
    want_qr: bool,
    want_barcode: bool,
    qr_modules: f32,
    code_modules: f32,
) -> Layout {
    let (width, height) = match format.page_mm {
        Some((w, h)) => (w, h),
        // Sheet labels are laid out by the grid and have room to grow
        // downwards, so only their width is constrained.
        None => (SHEET_WIDTH_MM / format.columns as f32, f32::MAX),
    };
    let avail_w = width - 2.0 * PAD_MM;
    let avail_h = if height == f32::MAX { f32::MAX } else { height - 2.0 * PAD_MM };

    let mut place = BarcodePlace::Below;
    let mut qr_mm = format.qr_mm.min(avail_h);

    if want_barcode && want_qr && avail_h != f32::MAX {
        let below_qr = (avail_h - BAR_HEIGHT_MM - GAP_MM).min(format.qr_mm);
        let beside_qr = format.qr_mm.min(avail_h);
        let beside_bar = (avail_w - beside_qr - GAP_MM) / code_modules;
        // Only move the barcode alongside if it stays scannable there and the
        // QR actually gains from it.
        if beside_bar >= MIN_BAR_MM && beside_qr > below_qr {
            place = BarcodePlace::Beside;
            qr_mm = beside_qr;
        } else {
            qr_mm = below_qr;
        }
    }
    qr_mm = qr_mm.max(8.0);

    let bar_mm = if !want_barcode {
        0.0
    } else {
        match place {
            BarcodePlace::Beside => (avail_w - qr_mm - GAP_MM) / code_modules,
            BarcodePlace::Below => avail_w / code_modules,
        }
    };

    // What's left for text once the symbols have taken their share.
    let text_height = match (want_barcode, place, avail_h) {
        (_, _, h) if h == f32::MAX => f32::MAX,
        (true, BarcodePlace::Below, h) => h - qr_mm.max(0.0) - BAR_HEIGHT_MM - GAP_MM,
        (_, _, h) => h - 6.0,
    };
    let roomy = text_height == f32::MAX || text_height > 14.0;

    Layout {
        qr_mm,
        qr_module_mm: if want_qr { qr_mm / qr_modules } else { 0.0 },
        bar_mm,
        place,
        max_items: if roomy { format.max_items } else { 0 },
        show_location: format.show_location && (roomy || !want_barcode),
    }
}

/// The label stock the print page knows how to lay out.
pub async fn label_formats() -> Json<serde_json::Value> {
    Json(json!(LABEL_FORMATS
        .iter()
        .map(|f| json!({
            "id": f.id,
            "name": f.name,
            "page_mm": f.page_mm.map(|(w, h)| json!([w, h])),
            "roll": f.page_mm.is_some(),
            "shows_contents": f.show_contents,
        }))
        .collect::<Vec<_>>()))
}

/// Renders a printable sheet or roll of labels. Each label carries a QR code
/// that opens that container's page, plus — where the label is big enough — a
/// barcode for laser scanners and a preview of what's inside.
pub async fn print_labels(
    State(st): State<AppState>,
    Query(params): Query<LabelParams>,
) -> AppResult<Html<String>> {
    let base = st.public_url();
    let requested = params.format.clone().unwrap_or_else(|| match params.size.as_deref() {
        Some("small") => "sheet-small".to_string(),
        _ => "sheet-large".to_string(),
    });
    let custom;
    let format = match find_format(&requested) {
        Some(f) => f,
        None if requested == "custom" => {
            custom = custom_format(params.w.unwrap_or(25.4), params.h.unwrap_or(25.4));
            &custom
        }
        None => find_format("sheet-large").unwrap(),
    };
    let on_paper = format.page_mm.is_none();

    let wanted: Option<Vec<String>> = params.codes.as_ref().filter(|_| !params.all).map(|c| {
        c.split(',').map(store::normalize_code).filter(|s| !s.is_empty()).collect()
    });

    let label_width = format.page_mm.map(|(w, _)| w).unwrap_or(SHEET_WIDTH_MM / format.columns as f32);
    let symbols = params.symbols.as_deref().unwrap_or("auto");
    let want_qr = matches!(symbols, "auto" | "qr" | "both");
    let mut want_barcode = match symbols {
        "barcode" | "both" => true,
        "auto" => label_width >= 48.0,
        _ => false,
    };
    // Cut-and-tape margins only make sense on paper you cut up yourself.
    let tape = on_paper && params.tape.as_deref() != Some("off");

    let all_containers = db::run(&st.pool, {
        let wanted = wanted.clone();
        move |c| {
            let all = store::all_containers(c)?;
            Ok(match &wanted {
                Some(codes) if !codes.is_empty() => {
                    all.into_iter().filter(|x| codes.contains(&x.code.to_uppercase())).collect()
                }
                _ => all,
            })
        }
    })
    .await?;

    // Size the symbols from the actual data: a longer code needs more modules,
    // and every module has to fit inside the label.
    let qr_modules = all_containers
        .iter()
        .filter_map(|c| QrCode::new(format!("{}/b/{}", base, c.code).as_bytes()).ok())
        .map(|q| q.width() as f32 + 8.0)
        .fold(29.0_f32, f32::max);
    let code_modules = all_containers
        .iter()
        .filter_map(|c| crate::barcode::code128_modules(&scannable_code(c)).ok())
        .fold(132u32, u32::max) as f32;

    let mut layout = plan_layout(format, want_qr, want_barcode, qr_modules, code_modules);
    // Both symbols compete for the same millimetres. Left to itself, `auto`
    // would happily fit a barcode by squeezing the QR past the point a phone
    // can read it — so if that is the trade on this stock, keep the QR.
    if symbols == "auto"
        && want_barcode
        && (layout.qr_module_mm < MIN_QR_MODULE_MM || layout.bar_mm < MIN_BAR_MM)
    {
        want_barcode = false;
        layout = plan_layout(format, want_qr, want_barcode, qr_modules, code_modules);
    }

    let max_items = layout.max_items as i64;
    let containers = db::run(&st.pool, move |c| {
        let mut out = Vec::new();
        for container in all_containers {
            let items = if max_items > 0 {
                store::query_items(
                    c,
                    &store::ItemQuery {
                        container_id: Some(container.id),
                        limit: Some(max_items),
                        ..Default::default()
                    },
                )?
            } else {
                Vec::new()
            };
            out.push((container, items));
        }
        Ok(out)
    })
    .await?;

    if containers.is_empty() {
        return Ok(Html(page_shell(
            "No labels",
            format,
            &layout,
            tape,
            "<p class=\"empty\">No containers matched. Add a box first, then print its label.</p>"
                .into(),
        )));
    }

    let mut body = String::new();
    body.push_str(&toolbar(format, symbols, want_qr, want_barcode, &layout, on_paper, tape));
    body.push_str(&format!("<div class=\"sheet {}\">", if on_paper { "paper" } else { "roll" }));

    for (container, items) in &containers {
        let url = format!("{}/b/{}", base, container.code);
        let qr = if want_qr {
            format!(
                "<div class=\"qr\" data-modules=\"{:.0}\" data-mm=\"{:.1}\">{}</div>",
                qr_modules,
                layout.qr_mm,
                qr_svg(&url, (layout.qr_mm * 8.0) as u32)?
            )
        } else {
            String::new()
        };
        // No caption under the bars: the code is already printed beside them.
        let barcode = if want_barcode {
            match crate::barcode::code128_svg(&scannable_code(container), 26) {
                Ok(svg) => format!("<div class=\"barcode\">{svg}</div>"),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };

        let contents = if layout.max_items > 0 && !items.is_empty() {
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
                    "<li class=\"more\">+{} more — scan for the full list</li>",
                    container.item_count - items.len() as i64
                )
            } else {
                String::new()
            };
            format!("<ul class=\"contents\">{lis}{more}</ul>")
        } else {
            String::new()
        };

        let location = if layout.show_location && container.path.contains(" / ") {
            let parent = container
                .path
                .rsplit_once(" / ")
                .map(|(head, _)| head.to_string())
                .unwrap_or_default();
            format!("<div class=\"where\">{}</div>", escape_html(&parent))
        } else {
            String::new()
        };
        let name = if format.show_name {
            format!("<div class=\"name\">{}</div>", escape_html(&container.name))
        } else {
            String::new()
        };

        let beside = layout.place == BarcodePlace::Beside;
        let tape_top = if tape {
            "<div class=\"tape-zone top\">cut here · tape over this strip</div>"
        } else {
            ""
        };
        let tape_bottom = if tape {
            "<div class=\"tape-zone bottom\">cut here · tape over this strip</div>"
        } else {
            ""
        };

        body.push_str(&format!(
            r#"<div class="label{stacked}{barcode_only}{taped}">
                 {tape_top}
                 <div class="label-main">
                   {qr}
                   <div class="meta">
                     <div class="code">{code}</div>
                     {name}
                     {location}
                     {contents}
                     {meta_barcode}
                   </div>
                 </div>
                 {below_barcode}
                 {tape_bottom}
               </div>"#,
            stacked = if format.stacked { " stacked" } else { "" },
            barcode_only = if want_barcode && !want_qr { " barcode-only" } else { "" },
            taped = if tape { " taped" } else { "" },
            tape_top = tape_top,
            qr = qr,
            code = escape_html(&container.code),
            name = name,
            location = location,
            contents = contents,
            meta_barcode = if beside { barcode.as_str() } else { "" },
            below_barcode = if beside { "" } else { barcode.as_str() },
            tape_bottom = tape_bottom,
        ));
    }
    body.push_str("</div>");
    Ok(Html(page_shell("Labels", format, &layout, tape, body)))
}

#[allow(clippy::too_many_arguments)]
fn toolbar(
    format: &LabelFormat,
    symbols: &str,
    want_qr: bool,
    want_barcode: bool,
    layout: &Layout,
    on_paper: bool,
    tape: bool,
) -> String {
    let options: String = LABEL_FORMATS
        .iter()
        .map(|f| {
            format!(
                "<option value=\"{}\"{}>{}</option>",
                f.id,
                if f.id == format.id { " selected" } else { "" },
                escape_html(f.name)
            )
        })
        .collect();
    let symbol_options: String = [
        ("auto", "Automatic"),
        ("qr", "QR code only"),
        ("both", "QR code and barcode"),
        ("barcode", "Barcode only"),
    ]
    .iter()
    .map(|(id, label)| {
        format!(
            "<option value=\"{id}\"{}>{label}</option>",
            if *id == symbols { " selected" } else { "" }
        )
    })
    .collect();

    // Say plainly what this print will produce, rather than leaving the user to
    // discover an unscannable label after cutting it out.
    let mut notes = String::new();
    if want_qr {
        let class = if layout.qr_module_mm < MIN_QR_MODULE_MM { "help warn" } else { "help" };
        let verdict = if layout.qr_module_mm < MIN_QR_MODULE_MM {
            " — that is tight for a phone camera. Use larger stock, or print the barcode only \
             and scan it with a laser scanner."
        } else {
            " — comfortable for a phone camera."
        };
        notes.push_str(&format!(
            "<p class=\"{class}\">QR code prints {:.0} mm across, {:.2} mm per module{verdict}</p>",
            layout.qr_mm, layout.qr_module_mm
        ));
    }
    if want_barcode {
        let class = if layout.bar_mm < MIN_BAR_MM { "help warn" } else { "help" };
        let verdict = if layout.bar_mm < MIN_BAR_MM {
            " — too fine for most laser scanners, which want 0.33 mm or more. Use wider stock."
        } else {
            " — any 1D laser scanner should read it."
        };
        notes.push_str(&format!(
            "<p class=\"{class}\">Barcode bars print at {:.2} mm{verdict}</p>",
            layout.bar_mm
        ));
    }

    let stock_help = match format.page_mm {
        Some((w, h)) => format!(
            "<p class=\"help\">Printing to a label printer: pick the LabelWriter in the print \
             dialog, set the label size to <strong>{}</strong> ({w} × {h} mm), margins to none \
             and scale to 100% — turn off “fit to page”. One label prints per page.</p>",
            escape_html(format.name)
        ),
        None => format!(
            "<p class=\"help\">Print at 100% scale on ordinary paper.{}</p>",
            if tape {
                " Cut along the outer line — the marked strips top and bottom are spare margin, \
                 so a slightly wonky cut costs you nothing, and packing tape can go over them \
                 without covering the codes."
            } else {
                " Cut carefully: without margins there is nothing between the outline and the \
                 printing."
            }
        ),
    };

    let tape_toggle = if on_paper {
        format!(
            "<label class=\"pick\"><input type=\"checkbox\" id=\"tape\"{}> Cut &amp; tape margins</label>",
            if tape { " checked" } else { "" }
        )
    } else {
        String::new()
    };

    format!(
        r#"<div class="toolbar no-print">
             <button onclick="window.print()">Print</button>
             <label class="pick">Label stock
               <select id="format">{options}<option value="custom"{custom}>Custom size…</option></select>
             </label>
             <label class="pick">Symbols
               <select id="symbols">{symbol_options}</select>
             </label>
             {tape_toggle}
             <a href="/#/labels">Back to the app</a>
           </div>
           {notes}
           {stock_help}
           <script>
             const reload = (changes) => {{
               const params = new URLSearchParams(location.search);
               for (const [key, value] of Object.entries(changes)) {{
                 if (value === null) params.delete(key);
                 else params.set(key, value);
               }}
               params.delete('size');
               location.search = params.toString();
             }};
             document.getElementById('format').addEventListener('change', (e) => {{
               const changes = {{ format: e.target.value }};
               if (e.target.value === 'custom') {{
                 const size = prompt('Label size in millimetres, width × height', '25 x 25');
                 if (!size) return;
                 const [w, h] = size.split(/[x×,]/).map((n) => parseFloat(n.trim()));
                 if (!w || !h) return;
                 changes.w = w;
                 changes.h = h;
               }}
               reload(changes);
             }});
             document.getElementById('symbols').addEventListener('change', (e) => {{
               reload({{ symbols: e.target.value }});
             }});
             const tapeBox = document.getElementById('tape');
             if (tapeBox) {{
               tapeBox.addEventListener('change', (e) => {{
                 reload({{ tape: e.target.checked ? 'on' : 'off' }});
               }});
             }}
           </script>"#,
        options = options,
        custom = if format.id == "custom" { " selected" } else { "" },
        symbol_options = symbol_options,
        tape_toggle = tape_toggle,
        notes = notes,
        stock_help = stock_help,
    )
}

fn page_shell(
    title: &str,
    format: &LabelFormat,
    layout: &Layout,
    tape: bool,
    body: String,
) -> String {
    // Roll stock prints one label per page at the exact label size; sheet stock
    // tiles labels onto whatever paper the printer holds.
    let (page_rule, label_rule, sheet_rule) = match format.page_mm {
        Some((w, h)) => (
            format!("@page {{ size: {w}mm {h}mm; margin: 0; }}"),
            format!(
                ".label {{ width: {w}mm; height: {h}mm; padding: {PAD_MM}mm; gap: {GAP_MM}mm;
                          border-radius: 0; overflow: hidden; }}
                 .label-main {{ gap: {GAP_MM}mm; }}
                 @media print {{ .label {{ break-after: page; page-break-after: always;
                                          border: 0; }} }}"
            ),
            ".sheet.roll { grid-template-columns: max-content; justify-content: center; }"
                .to_string(),
        ),
        None => (
            "@page { margin: 10mm; }".to_string(),
            format!(
                ".label {{ padding: {}; gap: 3mm; }}
                 .label-main {{ gap: 3mm; padding: {}; }}",
                if tape { "0" } else { "3mm" },
                // Side margin gives a crooked vertical cut the same slack the
                // tape strips give a horizontal one.
                if tape { "0 5mm" } else { "0" }
            ),
            format!(".sheet.paper {{ grid-template-columns: repeat({}, 1fr); }}", format.columns),
        ),
    };
    let qr = layout.qr_mm;
    let bar_height = BAR_HEIGHT_MM;
    format!(
        r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · Packrat</title>
<style>
  * {{ box-sizing: border-box; }}
  body {{ font: 15px/1.45 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
         margin: 0; padding: 16px; color: #111; background: #f6f7f9; }}
  .toolbar {{ display: flex; gap: 14px; align-items: center; margin-bottom: 10px; flex-wrap: wrap; }}
  .toolbar button {{ font: inherit; font-weight: 600; padding: 9px 16px; border-radius: 8px;
                     border: 0; background: #b45309; color: #fff; cursor: pointer; }}
  .toolbar a {{ color: #1d4ed8; }}
  .toolbar .pick {{ display: flex; align-items: center; gap: 6px; font-size: 13px; color: #555; }}
  .toolbar select {{ font: inherit; font-size: 14px; padding: 6px 8px; border-radius: 8px;
                     border: 1px solid #ccc; background: #fff; color: #111; }}
  .help {{ color: #555; font-size: 13px; max-width: 70ch; margin: 0 0 6px; }}
  .help.warn {{ color: #8a5300; font-weight: 600; }}
  .empty {{ color: #666; }}
  .sheet {{ display: grid; gap: 10px; margin-top: 12px; }}
  {sheet_rule}
  .label {{ display: flex; flex-direction: column; border: 1px dashed #bbb; border-radius: 8px;
            background: #fff; break-inside: avoid; page-break-inside: avoid; }}
  /* The barcode goes full width under everything else unless the label is wide
     and short, where sitting beside the QR leaves the QR its full height. */
  .label-main {{ display: flex; align-items: flex-start; flex: 1 1 auto;
                 min-height: 0; width: 100%; overflow: hidden; }}
  .label.stacked .label-main {{ flex-direction: column; align-items: center; text-align: center; }}
  {label_rule}
  {page_rule}
  .label .qr {{ flex: 0 0 auto; }}
  .label .qr svg {{ display: block; width: {qr}mm; height: {qr}mm; }}
  .meta {{ min-width: 0; overflow: hidden; flex: 1 1 auto; }}
  .label.stacked .meta {{ width: 100%; }}
  .code {{ font: 700 9pt ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .4px;
           white-space: nowrap; }}
  .label.stacked .code {{ font-size: 7.5pt; }}
  .name {{ font-weight: 700; font-size: 11pt; margin-top: .4mm; line-height: 1.15;
           overflow-wrap: anywhere; }}
  .label.stacked .name {{ font-size: 6.5pt; white-space: nowrap; overflow: hidden;
                          text-overflow: ellipsis; }}
  .where {{ color: #555; font-size: 7.5pt; margin-top: .4mm; white-space: nowrap;
            overflow: hidden; text-overflow: ellipsis; }}
  .contents {{ margin: 1.4mm 0 0; padding-left: 4mm; font-size: 7.5pt; color: #222;
               line-height: 1.25; }}
  .contents li {{ margin: .2mm 0; }}
  .contents .qty {{ color: #555; }}
  .contents .more {{ color: #777; font-style: italic; list-style: none; margin-left: -4mm; }}
  .barcode {{ margin-top: 1.2mm; width: 100%; }}
  .barcode svg {{ display: block; width: 100%; height: {bar_height}mm; }}
  .label.barcode-only .barcode svg {{ height: 12mm; }}
  /* Spare margin at the top and bottom of a paper label: room for a wonky cut,
     and somewhere to run tape without covering a code. Drawn with borders and
     text only, because browsers drop background graphics when printing. */
  .tape-zone {{ flex: 0 0 auto; height: 8mm; display: flex; align-items: center;
                justify-content: center; font-size: 6pt; letter-spacing: .14em;
                text-transform: uppercase; color: #aaa; }}
  .tape-zone.top {{ border-bottom: 1px dotted #ddd; }}
  .tape-zone.bottom {{ border-top: 1px dotted #ddd; }}
  .label.taped {{ border-style: solid; border-color: #999; }}
  @media print {{
    body {{ background: #fff; padding: 0; }}
    .no-print, .help {{ display: none !important; }}
    .sheet {{ gap: 0; margin-top: 0; }}
    .tape-zone {{ color: #bbb; }}
  }}
</style>
</head><body>{body}</body></html>"#
    )
}
