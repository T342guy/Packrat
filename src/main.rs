// SPDX-License-Identifier: GPL-3.0-only
//! Command line entry point: parses options, opens the database and serves.

use packrat::{db, logging, net, router, seed_example, AppState};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

struct Args {
    port: u16,
    host: String,
    database: PathBuf,
    public_url: Option<String>,
    seed_example: bool,
    /// Level for our own logs: off, error, warn, info, debug or trace.
    log_level: String,
    /// Watch a running instance's logs instead of starting a server.
    hook_logging: bool,
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
    -v, -V, --version       Print version and licensing

LOGGING:
        --start-with-logging  Serve with logging turned up (debug): every request,
                              every scan, every database migration
        --log <LEVEL>         Serve at a specific level: off, error, warn, info,
                              debug, trace [default: warn]
        --hook-logging        Do not serve. Attach to a Packrat already running on
                              this machine — one started by systemd, launchd or
                              Docker — and print its logs live until Ctrl-C.
                              Uses --port to find it.

ENVIRONMENT:
    Every option can be set by environment variable instead, which is usually
    easier in a container. Command line flags win over environment variables.

    PACKRAT_PORT, PACKRAT_HOST, PACKRAT_DB, PACKRAT_PUBLIC_URL, PACKRAT_SEED_EXAMPLE
    PACKRAT_LOG_LEVEL   same values as --log

    PACKRAT_LOG accepts the full RUST_LOG syntax and overrides --log, e.g.
    PACKRAT_LOG=packrat=debug,axum=info
";

/// Reads an option from the environment, ignoring blanks so an empty variable
/// in a compose file behaves the same as an unset one.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// True when we appear to be inside a container, where the address the server
/// detects for itself is not one any phone can reach.
fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|c| c.contains("docker") || c.contains("containerd") || c.contains("kubepods"))
            .unwrap_or(false)
}

fn parse_args() -> Result<Args, String> {
    // Environment first, command line second — flags override the environment.
    let mut args = Args {
        port: match env_var("PACKRAT_PORT") {
            Some(p) => p
                .parse()
                .map_err(|_| "PACKRAT_PORT must be a number".to_string())?,
            None => 8080,
        },
        host: env_var("PACKRAT_HOST").unwrap_or_else(|| "0.0.0.0".to_string()),
        database: PathBuf::from(
            env_var("PACKRAT_DB").unwrap_or_else(|| "inventory.db".to_string()),
        ),
        public_url: env_var("PACKRAT_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()),
        seed_example: matches!(
            env_var("PACKRAT_SEED_EXAMPLE").as_deref(),
            Some("1" | "true" | "yes" | "on")
        ),
        log_level: env_var("PACKRAT_LOG_LEVEL").unwrap_or_else(|| "warn".to_string()),
        hook_logging: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-v" | "-V" | "--version" => {
                // The shape GNU tools use: version, then the notices the GPL
                // asks for.
                println!("packrat {}", packrat::VERSION);
                println!("{}", packrat::COPYRIGHT);
                println!("License GPLv3: GNU GPL version 3 <https://gnu.org/licenses/gpl.html>");
                println!("This is free software: you are free to change and redistribute it.");
                println!("There is NO WARRANTY, to the extent permitted by law.");
                std::process::exit(0);
            }
            "-p" | "--port" => {
                args.port = value()?
                    .parse()
                    .map_err(|_| "port must be a number".to_string())?
            }
            "--host" => args.host = value()?,
            "-d" | "--db" | "--database" => args.database = PathBuf::from(value()?),
            "--public-url" => {
                args.public_url = Some(value()?.trim_end_matches('/').to_string());
            }
            "--seed-example" => args.seed_example = true,
            "--start-with-logging" => args.log_level = "debug".to_string(),
            "--log" => args.log_level = value()?,
            "--hook-logging" => args.hook_logging = true,
            other => return Err(format!("unknown option {other}\n\n{HELP}")),
        }
    }
    Ok(args)
}

#[tokio::main]
async fn main() {
    // Rust ignores SIGPIPE, so writing into a closed pipe returns an error and
    // println! panics. `packrat --version | head -1` failed roughly one time in
    // ten that way, depending on whether the reader had exited yet. Restoring
    // the default handler makes the process end quietly, the way every other
    // command line tool does.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    if let Err(message) = run().await {
        eprintln!("packrat: {message}");
        std::process::exit(1);
    }
}

/// Attaches to a running instance and prints its log stream.
///
/// Speaks just enough HTTP to read a chunked server-sent-event body, which
/// avoids pulling an HTTP client into the binary for one debugging command.
async fn hook_logging(port: u16) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let address = format!("127.0.0.1:{port}");
    let mut stream = tokio::net::TcpStream::connect(&address)
        .await
        .map_err(|e| {
            format!(
                "nothing is answering on {address}: {e}\n\n\
             Is Packrat running? Check with `systemctl status packrat`, or start it\n\
             yourself with `packrat --start-with-logging`. If it is on another port,\n\
             pass --port."
            )
        })?;
    stream
        .write_all(
            format!(
                "GET /api/logs/stream HTTP/1.1\r\nHost: {address}\r\n\
                 Accept: text/event-stream\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| format!("could not ask for the log stream: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| format!("no reply from {address}: {e}"))?;
    if !line.contains(" 200 ") {
        return Err(format!("{address} answered with {}", line.trim()));
    }
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).await.unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
    }

    println!("\n  Watching Packrat on {address}. Press Ctrl-C to stop.");
    println!("  Nothing will appear until it logs something — try loading a page.\n");

    // Chunked transfer encoding: a hex length, the bytes, then CRLF.
    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line).await.unwrap_or(0) == 0 {
            break;
        }
        if size_line.trim().is_empty() {
            continue;
        }
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        let mut chunk = vec![0u8; size + 2];
        if reader.read_exact(&mut chunk).await.is_err() {
            break;
        }
        for line in String::from_utf8_lossy(&chunk[..size]).lines() {
            if let Some(message) = line.strip_prefix("data: ") {
                println!("{message}");
            }
        }
    }
    println!("\n  The stream ended — Packrat probably stopped.");
    Ok(())
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;

    if args.hook_logging {
        return hook_logging(args.port).await;
    }
    logging::init(&args.log_level);
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
        db::get_setting(&conn, "public_url")
            .map_err(|e| e.to_string())?
            .filter(|s| !s.is_empty())
    };
    let state = AppState {
        pool,
        port: args.port,
        lan_ip: net::lan_ip(),
        public_url_override: Arc::new(RwLock::new(args.public_url.clone().or(stored))),
    };

    // Report a clock that has moved backwards: check-up ages are measured
    // against it, and a wrong clock would quietly make everything look fresh.
    {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        match packrat::store::clock_status(&conn) {
            Ok(status) => {
                if let Some(behind) = status.behind_seconds {
                    let days = behind / 86_400;
                    tracing::warn!(behind_seconds = behind, "system clock is behind");
                    println!(
                        "\n  ⚠ This machine's clock reads earlier than the last time Packrat saw\n    \
                         ({} behind). Check-up ages are held at the later of the two, so nothing\n    \
                         is wrongly marked fresh — but fixing the clock is worth doing.",
                        if days > 0 {
                            format!("about {days} days")
                        } else {
                            format!("{} minutes", behind / 60)
                        }
                    );
                }
            }
            Err(e) => eprintln!("could not read the clock state: {}", e.message),
        }
        let _ = packrat::store::touch_clock(&conn);
    }
    packrat::spawn_clock_keeper(state.pool.clone());

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
        println!(
            "  network    http://{}:{}   ← open this on your phone",
            ip, args.port
        );
    }
    println!("  QR links   {}", state.public_url());
    // In a container the detected address belongs to the container network, so
    // QR codes would point somewhere no phone can reach. Say so loudly.
    if in_container()
        && state
            .public_url_override
            .read()
            .map(|u| u.is_none())
            .unwrap_or(false)
    {
        println!(
            "\n  ⚠ Running in a container with no public URL set, so QR codes point at\n    \
             {} — an address inside the container network that phones cannot reach.\n    \
             Set PACKRAT_PUBLIC_URL to the host's address, e.g. http://192.168.1.24:{}",
            state.public_url(),
            args.port
        );
    }
    println!("\n  No password is required, so keep this on a network you trust.");
    println!("  Press Ctrl-C to stop.\n");

    tracing::info!(
        port = args.port,
        database = %absolute.display(),
        version = env!("CARGO_PKG_VERSION"),
        "packrat started"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("server error: {e}"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\n  Shutting down. Your inventory is saved.");
}
