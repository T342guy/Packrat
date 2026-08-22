// SPDX-License-Identifier: GPL-3.0-only
//! Packrat — a small, locally-hostable inventory server.
//!
//! Everything lives in one SQLite file and one binary: run it on a machine at
//! home, open it from any phone or laptop on the same network, and scan the QR
//! code on a box to see what's inside without opening it.
//!
//! The crate is a library with a thin binary on top so that benchmarks and
//! integration tests can drive the same code the server runs.

/// The notices GPLv3 section 5(d) asks an interactive interface to display.
/// Kept in one place so the command line, the API and the web footer cannot
/// drift apart.
pub const COPYRIGHT: &str = "Copyright © 2026 T342guy";
pub const LICENSE_NAME: &str = "GPL-3.0-only";
pub const SOURCE_URL: &str = "https://github.com/T342guy/packrat";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod api;
pub mod backup;
pub mod barcode;
pub mod db;
pub mod error;
pub mod logging;
pub mod media;
pub mod models;
pub mod net;
pub mod store;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub pool: db::Pool,
    pub port: u16,
    pub lan_ip: Option<IpAddr>,
    /// Base URL baked into QR codes. Configurable at runtime from Settings.
    pub public_url_override: Arc<RwLock<Option<String>>>,
}

impl AppState {
    /// The URL this server is most likely reachable at from a phone.
    pub fn detected_url(&self) -> String {
        match self.lan_ip {
            Some(ip) => format!("http://{}:{}", ip, self.port),
            None => format!("http://localhost:{}", self.port),
        }
    }

    /// The base URL QR codes point at.
    pub fn public_url(&self) -> String {
        self.public_url_override
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| self.detected_url())
    }
}

/// One line per request, at debug level: quiet by default, useful the moment
/// someone turns logging up to work out why a scanner or a phone is unhappy.
async fn log_requests(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let millis = started.elapsed().as_millis();
    if status >= 500 {
        tracing::error!(%method, %path, status, millis, "request failed");
    } else if status >= 400 {
        tracing::warn!(%method, %path, status, millis, "request rejected");
    } else {
        tracing::debug!(%method, %path, status, millis, "request");
    }
    response
}

/// Keeps the clock high-water mark moving while the server runs, so a machine
/// whose clock later jumps backwards can be spotted.
pub fn spawn_clock_keeper(pool: db::Pool) {
    tokio::spawn(async move {
        loop {
            if let Ok(conn) = pool.get() {
                if let Err(error) = store::touch_clock(&conn) {
                    tracing::warn!(%error, "could not record the time");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    });
}

pub fn router(state: AppState) -> Router {
    Router::new()
        // Frontend
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/icon.svg", get(icon_svg))
        .route("/manifest.webmanifest", get(manifest))
        // Served from the binary so "how to view a copy of this License" holds
        // on a machine with no internet connection, which is the normal case
        // for something running in a garage.
        .route("/license", get(license))
        // What a QR code on a box resolves to.
        .route("/b/{code}", get(scan_redirect))
        .route("/labels", get(media::print_labels))
        .route("/api/label-formats", get(media::label_formats))
        // Containers
        .route(
            "/api/containers",
            get(api::list_containers).post(api::create_container),
        )
        .route("/api/containers/{id}", get(api::get_container))
        .route("/api/containers/{id}", put(api::update_container))
        .route("/api/containers/{id}", delete(api::delete_container))
        .route("/api/containers/{id}/qr.svg", get(media::container_qr))
        .route("/api/containers/{id}/verify", post(api::verify_container))
        .route("/api/containers/{id}/grid", put(api::set_container_grid))
        .route(
            "/api/containers/{id}/position",
            put(api::set_container_position),
        )
        .route(
            "/api/containers/{id}/barcode.svg",
            get(media::container_barcode),
        )
        .route("/api/scan/{code}", get(api::scan))
        .route("/api/stale", get(api::stale_containers))
        .route("/api/by-code/{code}", get(api::get_container_by_code))
        // Items
        .route("/api/items", get(api::list_items).post(api::create_item))
        .route("/api/items/{id}", get(api::get_item))
        .route("/api/items/{id}", put(api::update_item))
        .route("/api/items/{id}", delete(api::delete_item))
        .route("/api/items/{id}/move", post(api::move_item))
        .route("/api/items/{id}/quantity", post(api::adjust_quantity))
        .route("/api/items/bulk-move", post(api::bulk_move))
        // Lookup and meta
        .route("/api/search", get(api::search))
        .route("/api/tags", get(api::list_tags))
        .route(
            "/api/tags/{name}",
            put(api::rename_tag).delete(api::delete_tag),
        )
        .route("/api/stats", get(api::stats))
        .route("/api/bootstrap", get(api::bootstrap))
        .route("/api/health", get(api::health))
        .route("/api/logs/stream", get(api::log_stream))
        .route(
            "/api/settings",
            get(api::get_settings).put(api::update_settings),
        )
        // Photos
        .route("/api/photos", post(media::upload_photo))
        .route("/photos/{id}", get(media::get_photo))
        // Backup
        .route("/api/export", get(backup::export_json))
        .route("/api/export.csv", get(backup::export_csv))
        .route("/api/import", post(backup::import_json))
        .fallback(not_found)
        .layer(axum::middleware::from_fn(log_requests))
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
        .with_state(state)
}

async fn scan_redirect(axum::extract::Path(code): axum::extract::Path<String>) -> Redirect {
    let safe: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    Redirect::to(&format!("/#/box/{safe}"))
}

async fn not_found(uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": format!("no route for {}", uri.path()) })),
        )
            .into_response();
    }
    // Anything else is a deep link into the single-page app.
    index().await.into_response()
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn index() -> Response {
    asset(
        "text/html; charset=utf-8",
        include_str!("../static/index.html"),
    )
}
async fn app_js() -> Response {
    asset(
        "application/javascript; charset=utf-8",
        include_str!("../static/app.js"),
    )
}
async fn styles_css() -> Response {
    asset(
        "text/css; charset=utf-8",
        include_str!("../static/styles.css"),
    )
}
async fn icon_svg() -> Response {
    asset("image/svg+xml", include_str!("../static/icon.svg"))
}
async fn license() -> Response {
    asset("text/plain; charset=utf-8", include_str!("../LICENSE"))
}
async fn manifest() -> Response {
    asset(
        "application/manifest+json",
        include_str!("../static/manifest.webmanifest"),
    )
}

/// Fills an empty database with a plausible garage so the app has something to
/// show on the first run.
pub fn seed_example(conn: &mut rusqlite::Connection) -> Result<bool, String> {
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM containers", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if existing > 0 {
        return Ok(false);
    }
    let make_container = |conn: &rusqlite::Connection,
                          name: &str,
                          kind: &str,
                          parent: Option<i64>|
     -> Result<i64, String> {
        let input = models::ContainerInput {
            name: name.to_string(),
            kind: kind.to_string(),
            parent_id: parent,
            notes: String::new(),
            photo_id: None,
            code: None,
            barcode: None,
        };
        store::create_container(conn, &input)
            .map(|c| c.id)
            .map_err(|e| e.message)
    };

    let garage = make_container(conn, "Garage", "area", None)?;
    let shelves = make_container(conn, "North wall shelving", "shelf", Some(garage))?;
    let workbench = make_container(conn, "Workbench cabinet", "cabinet", Some(garage))?;
    let camping = make_container(conn, "Camping gear", "box", Some(shelves))?;
    let holiday = make_container(conn, "Holiday decorations", "bin", Some(shelves))?;
    let fasteners = make_container(conn, "Fasteners", "drawer", Some(workbench))?;

    let seed_items: &[(&str, &str, i64, i64, &[&str])] = &[
        (
            "4-person tent",
            "Blue dome tent, poles in side pocket",
            1,
            camping,
            &["camping", "outdoors"],
        ),
        (
            "Sleeping bags",
            "Two mummy bags, rated 0°C",
            2,
            camping,
            &["camping"],
        ),
        (
            "Camping stove",
            "Propane, needs a full canister",
            1,
            camping,
            &["camping", "cooking"],
        ),
        (
            "String lights",
            "Warm white, 3 strands, one has a dead bulb",
            3,
            holiday,
            &["holiday"],
        ),
        (
            "Wreath",
            "Front door wreath in a paper sleeve",
            1,
            holiday,
            &["holiday"],
        ),
        (
            "Wood screws #8 x 1½\"",
            "Roughly half a box left",
            1,
            fasteners,
            &["hardware", "screws"],
        ),
        (
            "Drywall anchors",
            "Assorted sizes in a plastic case",
            1,
            fasteners,
            &["hardware"],
        ),
        (
            "Cordless drill",
            "Battery on the charger by the door",
            1,
            workbench,
            &["tools", "power-tools"],
        ),
        (
            "Extension cord 25ft",
            "Orange, heavy gauge",
            2,
            workbench,
            &["tools", "electrical"],
        ),
    ];
    for (name, description, quantity, container, tags) in seed_items {
        let input = models::ItemInput {
            name: name.to_string(),
            description: description.to_string(),
            quantity: *quantity,
            container_id: Some(*container),
            photo_id: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            barcode: None,
        };
        store::create_item(conn, &input).map_err(|e| e.message)?;
    }
    Ok(true)
}
