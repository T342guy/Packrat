//! Packrat — a small, locally-hostable inventory server.
//!
//! Everything lives in one SQLite file and one binary: run it on a machine at
//! home, open it from any phone or laptop on the same network, and scan the QR
//! code on a box to see what's inside without opening it.

mod api;
mod barcode;
mod backup;
mod db;
mod error;
mod media;
mod models;
mod net;
mod store;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
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

struct Args {
    port: u16,
    host: String,
    database: PathBuf,
    public_url: Option<String>,
    seed_example: bool,
}

const HELP: &str = "\
packrat — a locally-hostable inventory for garages, sheds and storage

USAGE:
    packrat [OPTIONS]

OPTIONS:
    -p, --port <PORT>       Port to listen on [default: 8080]
        --host <ADDR>       Address to bind [default: 0.0.0.0, i.e. reachable on your LAN]
    -d, --db <PATH>         SQLite database file [default: ./inventory.db]
        --public-url <URL>  Base URL to encode in QR codes (default: auto-detected LAN address)
        --seed-example      Populate an empty database with a small example inventory
    -h, --help              Print this help
    -V, --version           Print version
";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        port: 8080,
        host: "0.0.0.0".to_string(),
        database: PathBuf::from("inventory.db"),
        public_url: None,
        seed_example: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("packrat {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-p" | "--port" => {
                args.port = value()?.parse().map_err(|_| "port must be a number".to_string())?
            }
            "--host" => args.host = value()?,
            "-d" | "--db" | "--database" => args.database = PathBuf::from(value()?),
            "--public-url" => {
                args.public_url = Some(value()?.trim_end_matches('/').to_string());
            }
            "--seed-example" => args.seed_example = true,
            other => return Err(format!("unknown option {other}\n\n{HELP}")),
        }
    }
    Ok(args)
}

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("packrat: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let pool = db::open(&args.database)?;

    if args.seed_example {
        let mut conn = pool.get().map_err(|e| e.to_string())?;
        match seed_example(&mut conn) {
            Ok(true) => println!("Seeded the database with an example inventory."),
            Ok(false) => println!("Database already has data — skipping the example inventory."),
            Err(e) => return Err(format!("could not seed example data: {e}")),
        }
    }

    // A URL set on the command line wins; otherwise fall back to whatever was
    // saved in Settings, and finally to the detected LAN address.
    let stored = {
        let conn = pool.get().map_err(|e| e.to_string())?;
        db::get_setting(&conn, "public_url").map_err(|e| e.to_string())?.filter(|s| !s.is_empty())
    };
    let state = AppState {
        pool,
        port: args.port,
        lan_ip: net::lan_ip(),
        public_url_override: Arc::new(RwLock::new(args.public_url.clone().or(stored))),
    };

    let app = router(state.clone());
    let bind: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .map_err(|e| format!("invalid --host/--port: {e}"))?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("cannot bind {bind}: {e}"))?;

    let absolute = std::fs::canonicalize(&args.database).unwrap_or(args.database.clone());
    println!("\n  Packrat");
    println!("  ───────");
    println!("  database   {}", absolute.display());
    println!("  local      http://localhost:{}", args.port);
    if let Some(ip) = state.lan_ip {
        println!("  network    http://{}:{}   ← open this on your phone", ip, args.port);
    }
    println!("  QR links   {}", state.public_url());
    println!("\n  No password is required, so keep this on a network you trust.");
    println!("  Press Ctrl-C to stop.\n");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("server error: {e}"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\n  Shutting down. Your inventory is saved.");
}

fn router(state: AppState) -> Router {
    Router::new()
        // Frontend
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/icon.svg", get(icon_svg))
        .route("/manifest.webmanifest", get(manifest))
        // What a QR code on a box resolves to.
        .route("/b/{code}", get(scan_redirect))
        .route("/labels", get(media::print_labels))
        .route("/api/label-formats", get(media::label_formats))
        // Containers
        .route("/api/containers", get(api::list_containers).post(api::create_container))
        .route("/api/containers/{id}", get(api::get_container))
        .route("/api/containers/{id}", put(api::update_container))
        .route("/api/containers/{id}", delete(api::delete_container))
        .route("/api/containers/{id}/qr.svg", get(media::container_qr))
        .route("/api/containers/{id}/verify", post(api::verify_container))
        .route("/api/containers/{id}/barcode.svg", get(media::container_barcode))
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
        .route("/api/tags/{name}", put(api::rename_tag).delete(api::delete_tag))
        .route("/api/stats", get(api::stats))
        .route("/api/bootstrap", get(api::bootstrap))
        .route("/api/settings", get(api::get_settings).put(api::update_settings))
        // Photos
        .route("/api/photos", post(media::upload_photo))
        .route("/photos/{id}", get(media::get_photo))
        // Backup
        .route("/api/export", get(backup::export_json))
        .route("/api/export.csv", get(backup::export_csv))
        .route("/api/import", post(backup::import_json))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
        .with_state(state)
}

async fn scan_redirect(axum::extract::Path(code): axum::extract::Path<String>) -> Redirect {
    let safe: String =
        code.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
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
    asset("text/html; charset=utf-8", include_str!("../static/index.html"))
}
async fn app_js() -> Response {
    asset("application/javascript; charset=utf-8", include_str!("../static/app.js"))
}
async fn styles_css() -> Response {
    asset("text/css; charset=utf-8", include_str!("../static/styles.css"))
}
async fn icon_svg() -> Response {
    asset("image/svg+xml", include_str!("../static/icon.svg"))
}
async fn manifest() -> Response {
    asset("application/manifest+json", include_str!("../static/manifest.webmanifest"))
}

/// Fills an empty database with a plausible garage so the app has something to
/// show on the first run.
fn seed_example(conn: &mut rusqlite::Connection) -> Result<bool, String> {
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
        store::create_container(conn, &input).map(|c| c.id).map_err(|e| e.message)
    };

    let garage = make_container(conn, "Garage", "area", None)?;
    let shelves = make_container(conn, "North wall shelving", "shelf", Some(garage))?;
    let workbench = make_container(conn, "Workbench cabinet", "cabinet", Some(garage))?;
    let camping = make_container(conn, "Camping gear", "box", Some(shelves))?;
    let holiday = make_container(conn, "Holiday decorations", "bin", Some(shelves))?;
    let fasteners = make_container(conn, "Fasteners", "drawer", Some(workbench))?;

    let seed_items: &[(&str, &str, i64, i64, &[&str])] = &[
        ("4-person tent", "Blue dome tent, poles in side pocket", 1, camping, &["camping", "outdoors"]),
        ("Sleeping bags", "Two mummy bags, rated 0°C", 2, camping, &["camping"]),
        ("Camping stove", "Propane, needs a full canister", 1, camping, &["camping", "cooking"]),
        ("String lights", "Warm white, 3 strands, one has a dead bulb", 3, holiday, &["holiday"]),
        ("Wreath", "Front door wreath in a paper sleeve", 1, holiday, &["holiday"]),
        ("Wood screws #8 x 1½\"", "Roughly half a box left", 1, fasteners, &["hardware", "screws"]),
        ("Drywall anchors", "Assorted sizes in a plastic case", 1, fasteners, &["hardware"]),
        ("Cordless drill", "Battery on the charger by the door", 1, workbench, &["tools", "power-tools"]),
        ("Extension cord 25ft", "Orange, heavy gauge", 2, workbench, &["tools", "electrical"]),
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
