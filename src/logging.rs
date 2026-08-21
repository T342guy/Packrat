//! Logging.
//!
//! Every log line goes to stdout — which is where a service manager or Docker
//! expects it — and is also fanned out over a broadcast channel so a running
//! instance can be watched live with `packrat --hook-logging`, without needing
//! access to journalctl or `docker logs`.

use std::io::Write;
use std::sync::OnceLock;
use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

/// Bounded on purpose: a watcher that stalls drops old lines rather than
/// growing the server's memory.
const STREAM_CAPACITY: usize = 256;

static STREAM: OnceLock<broadcast::Sender<String>> = OnceLock::new();

pub fn channel() -> &'static broadcast::Sender<String> {
    STREAM.get_or_init(|| broadcast::channel(STREAM_CAPACITY).0)
}

/// Writes each log event to stdout and to the broadcast channel.
#[derive(Clone, Copy)]
struct Tee;

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeEvent;
    fn make_writer(&'a self) -> Self::Writer {
        TeeEvent { buffer: Vec::new() }
    }
}

/// One log event's bytes. The subscriber drops this when the event is
/// complete, which is the point at which a whole line can be broadcast.
struct TeeEvent {
    buffer: Vec<u8>,
}

impl Write for TeeEvent {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(data);
        std::io::stdout().write_all(data)?;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

impl Drop for TeeEvent {
    fn drop(&mut self) {
        if let Ok(line) = String::from_utf8(std::mem::take(&mut self.buffer)) {
            let line = line.trim_end();
            if !line.is_empty() {
                // Fails only when nobody is watching, which is the normal case.
                let _ = channel().send(line.to_string());
            }
        }
    }
}

/// Starts logging at the given level. `PACKRAT_LOG` overrides it and accepts
/// the full `RUST_LOG` syntax, e.g. `packrat=debug,axum=info`.
pub fn init(default_level: &str) {
    let filter = EnvFilter::try_from_env("PACKRAT_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("packrat={default_level},warn")));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(Tee)
        .with_target(false)
        .with_ansi(false)
        .try_init();
}
