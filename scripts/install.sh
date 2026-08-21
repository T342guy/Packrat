#!/usr/bin/env bash
# Installs Packrat and registers it to start automatically at boot.
#
#   Linux  (systemd)  — a system service when run with sudo, otherwise a user
#                       service with lingering enabled so it starts at boot
#                       without anyone logging in.
#   macOS  (launchd)  — a LaunchAgent.
#
# Nothing here touches your inventory database: uninstalling leaves it behind.

set -euo pipefail

APP="packrat"
PORT="8080"
HOST="0.0.0.0"
DB=""
PUBLIC_URL=""
BINARY=""
PREFIX=""
MODE=""          # system | user, auto-detected from whether we are root
DRY_RUN=0
UNINSTALL=0

usage() {
  cat <<'USAGE'
Usage: scripts/install.sh [options]

Installs the packrat binary and sets it to start at boot.

Options:
  -p, --port <PORT>       Port to serve on (default: 8080)
      --host <ADDR>       Address to bind (default: 0.0.0.0, reachable on your LAN)
  -d, --db <PATH>         Database file (default: per-platform state directory)
      --public-url <URL>  Base URL for QR codes (default: auto-detected at runtime)
      --binary <PATH>     Use this prebuilt binary instead of building one
      --prefix <DIR>      Install root (default: /usr/local for root, ~/.local otherwise)
      --system            Force a system-wide service (needs root)
      --user              Force a per-user service
  -n, --dry-run           Print what would happen, change nothing
      --uninstall         Stop and remove the service; keeps the database
  -h, --help              This message

Examples:
  sudo scripts/install.sh --port 8080          # system service, starts at boot
  scripts/install.sh --user                    # just for me, no root needed
  scripts/install.sh --dry-run                 # see the service file first
  sudo scripts/install.sh --uninstall
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    -p|--port) PORT="${2:?--port needs a value}"; shift 2 ;;
    --host) HOST="${2:?--host needs a value}"; shift 2 ;;
    -d|--db) DB="${2:?--db needs a value}"; shift 2 ;;
    --public-url) PUBLIC_URL="${2:?--public-url needs a value}"; shift 2 ;;
    --binary) BINARY="${2:?--binary needs a value}"; shift 2 ;;
    --prefix) PREFIX="${2:?--prefix needs a value}"; shift 2 ;;
    --system) MODE="system"; shift ;;
    --user) MODE="user"; shift ;;
    -n|--dry-run) DRY_RUN=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; echo >&2; usage >&2; exit 2 ;;
  esac
done

say()  { printf '%s\n' "$*"; }
step() { printf '  %s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

# Every mutating action goes through this, so --dry-run is honest by design.
run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '  would run: %s\n' "$*"
  else
    "$@"
  fi
}

write_file() {
  target="$1"
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '\n  would write %s:\n\n' "$target"
    sed 's/^/    /'
    printf '\n'
  else
    if [ -w "$(dirname "$target")" ]; then
      cat > "$target"
    else
      sudo tee "$target" >/dev/null
    fi
  fi
}

# ------------------------------------------------------------------ platform

OS="$(uname -s)"
case "$OS" in
  Linux)
    command -v systemctl >/dev/null 2>&1 || die \
      "this script only knows systemd on Linux. Run packrat manually, or use the container image."
    INIT="systemd" ;;
  Darwin) INIT="launchd" ;;
  *) die "unsupported system: $OS. The container image works anywhere Docker does." ;;
esac

if [ -z "$MODE" ]; then
  if [ "$(id -u)" -eq 0 ]; then MODE="system"; else MODE="user"; fi
fi
if [ "$MODE" = "system" ] && [ "$(id -u)" -ne 0 ]; then
  die "a system service needs root — re-run with sudo, or pass --user"
fi

if [ -z "$PREFIX" ]; then
  if [ "$MODE" = "system" ]; then PREFIX="/usr/local"; else PREFIX="$HOME/.local"; fi
fi
BIN_DIR="$PREFIX/bin"
BIN_PATH="$BIN_DIR/$APP"

if [ -z "$DB" ]; then
  if [ "$INIT" = "launchd" ]; then
    if [ "$MODE" = "system" ]; then
      DB="/Library/Application Support/Packrat/inventory.db"
    else
      DB="$HOME/Library/Application Support/Packrat/inventory.db"
    fi
  elif [ "$MODE" = "system" ]; then
    DB="/var/lib/$APP/inventory.db"
  else
    DB="${XDG_DATA_HOME:-$HOME/.local/share}/$APP/inventory.db"
  fi
fi
DB_DIR="$(dirname "$DB")"

SERVICE_USER="$APP"
UNIT_SYSTEM="/etc/systemd/system/$APP.service"
UNIT_USER="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$APP.service"
PLIST_LABEL="dev.packrat"
if [ "$INIT" = "launchd" ] && [ "$MODE" = "system" ]; then
  # A daemon starts at boot without anyone logging in; an agent needs a login
  # session. For a machine that lives in the garage, that difference matters.
  PLIST="/Library/LaunchDaemons/$PLIST_LABEL.plist"
  LAUNCH_DOMAIN="system"
else
  PLIST="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"
  LAUNCH_DOMAIN="gui/$(id -u)"
fi

# ----------------------------------------------------------------- uninstall

if [ "$UNINSTALL" -eq 1 ]; then
  say "Removing the Packrat service ($INIT, $MODE)."
  case "$INIT:$MODE" in
    systemd:system)
      run systemctl stop "$APP.service" || true
      run systemctl disable "$APP.service" || true
      run rm -f "$UNIT_SYSTEM"
      run systemctl daemon-reload ;;
    systemd:user)
      run systemctl --user stop "$APP.service" || true
      run systemctl --user disable "$APP.service" || true
      run rm -f "$UNIT_USER"
      run systemctl --user daemon-reload ;;
    launchd:*)
      run launchctl bootout "$LAUNCH_DOMAIN/$PLIST_LABEL" || true
      run rm -f "$PLIST" ;;
  esac
  run rm -f "$BIN_PATH"
  say ""
  say "Done. Your inventory was left untouched at:"
  say "  $DB"
  say "Delete it yourself if you really mean to."
  exit 0
fi

# -------------------------------------------------------------------- binary

say "Packrat installer"
say "  service    $INIT, $MODE-wide"
say "  binary     $BIN_PATH"
say "  database   $DB"
say "  address    http://$HOST:$PORT"
say ""

if [ -z "$BINARY" ]; then
  here="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
  if [ -x "$here/target/release/$APP" ]; then
    BINARY="$here/target/release/$APP"
    step "Using the release build already in this checkout."
  elif command -v cargo >/dev/null 2>&1 && [ -f "$here/Cargo.toml" ]; then
    step "Building from source (this takes a minute or two)…"
    if [ "$DRY_RUN" -eq 1 ]; then
      printf '  would run: cargo build --release --locked (in %s)\n' "$here"
    else
      ( cd "$here" && cargo build --release --locked )
    fi
    BINARY="$here/target/release/$APP"
  elif command -v "$APP" >/dev/null 2>&1; then
    BINARY="$(command -v "$APP")"
    step "Using the packrat already on your PATH: $BINARY"
  else
    die "no binary and no Rust toolchain.
Install Rust from https://rustup.rs and re-run, pass --binary <path> to a
prebuilt one, or skip all this and use the container image:
  docker run -d --name packrat -p $PORT:8080 -v packrat-data:/data \\
    -e PACKRAT_PUBLIC_URL=http://<this-machine>:$PORT ghcr.io/t342guy/packrat:latest"
  fi
fi

step "Installing the binary."
run mkdir -p "$BIN_DIR"
run install -m 0755 "$BINARY" "$BIN_PATH"

# --------------------------------------------------------------- the service

# An array, not a string: the conventional macOS data directory has a space in
# it, and a --db path from the command line might too.
EXEC_ARGS=(--db "$DB" --host "$HOST" --port "$PORT")
if [ -n "$PUBLIC_URL" ]; then
  EXEC_ARGS+=(--public-url "$PUBLIC_URL")
fi

# systemd accepts double-quoted arguments in ExecStart.
exec_line="$BIN_PATH"
for arg in "${EXEC_ARGS[@]}"; do
  case "$arg" in
    *[[:space:]]*) exec_line="$exec_line \"$arg\"" ;;
    *) exec_line="$exec_line $arg" ;;
  esac
done

# systemd splits directives on whitespace unless the value is quoted.
case "$DB_DIR" in
  *[[:space:]]*) rw_path="\"$DB_DIR\"" ;;
  *) rw_path="$DB_DIR" ;;
esac

xml_escape() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

case "$INIT:$MODE" in
  systemd:system)
    step "Creating the '$SERVICE_USER' service account."
    if [ "$DRY_RUN" -eq 1 ]; then
      printf '  would run: useradd --system --home-dir /var/lib/%s --shell /usr/sbin/nologin %s\n' \
        "$APP" "$SERVICE_USER"
    elif ! id "$SERVICE_USER" >/dev/null 2>&1; then
      useradd --system --home-dir "/var/lib/$APP" --shell /usr/sbin/nologin "$SERVICE_USER" \
        || die "could not create the $SERVICE_USER account"
    fi
    run mkdir -p "$DB_DIR"
    run chown "$SERVICE_USER:$SERVICE_USER" "$DB_DIR"

    step "Writing $UNIT_SYSTEM"
    write_file "$UNIT_SYSTEM" <<UNIT
[Unit]
Description=Packrat inventory
Documentation=https://github.com/T342guy/packrat
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
ExecStart=$exec_line
Restart=on-failure
RestartSec=5

# It serves a local web app off one SQLite file; it needs nothing else.
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ProtectControlGroups=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
ReadWritePaths=$rw_path

[Install]
WantedBy=multi-user.target
UNIT
    run systemctl daemon-reload
    run systemctl enable --now "$APP.service"
    STATUS_CMD="systemctl status $APP"
    LOG_CMD="journalctl -u $APP -f"
    ;;

  systemd:user)
    run mkdir -p "$DB_DIR" "$(dirname "$UNIT_USER")"
    step "Writing $UNIT_USER"
    write_file "$UNIT_USER" <<UNIT
[Unit]
Description=Packrat inventory
Documentation=https://github.com/T342guy/packrat
After=network-online.target

[Service]
Type=simple
ExecStart=$exec_line
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=default.target
UNIT
    run systemctl --user daemon-reload
    run systemctl --user enable --now "$APP.service"
    # Without lingering a user service stops when you log out and does not come
    # back at boot — which defeats the point on a machine in the garage.
    step "Enabling lingering so it survives logout and starts at boot."
    if [ "$DRY_RUN" -eq 1 ]; then
      printf '  would run: loginctl enable-linger %s\n' "$(id -un)"
    else
      loginctl enable-linger "$(id -un)" 2>/dev/null \
        || say "  note: could not enable lingering (needs root). It will start when you log in."
    fi
    STATUS_CMD="systemctl --user status $APP"
    LOG_CMD="journalctl --user -u $APP -f"
    ;;

  launchd:*)
    if [ "$MODE" = "system" ]; then
      LOG_DIR="/Library/Logs"
    else
      LOG_DIR="$HOME/Library/Logs"
    fi
    run mkdir -p "$DB_DIR" "$(dirname "$PLIST")" "$LOG_DIR"
    step "Writing $PLIST"
    ARGS_XML=""
    for arg in "${EXEC_ARGS[@]}"; do
      ARGS_XML="$ARGS_XML
    <string>$(xml_escape "$arg")</string>"
    done
    RUN_AS=""
    if [ "$MODE" = "system" ]; then
      # Run as the human who invoked sudo rather than as root.
      RUN_AS="${SUDO_USER:-root}"
      ARGS_XML="$ARGS_XML"
    fi
    write_file "$PLIST" <<PLISTFILE
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$PLIST_LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_PATH</string>$ARGS_XML
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>${RUN_AS:+
  <key>UserName</key>
  <string>$RUN_AS</string>}
  <key>StandardOutPath</key>
  <string>$LOG_DIR/packrat.log</string>
  <key>StandardErrorPath</key>
  <string>$LOG_DIR/packrat.log</string>
</dict>
</plist>
PLISTFILE
    run launchctl bootout "$LAUNCH_DOMAIN/$PLIST_LABEL" || true
    run launchctl bootstrap "$LAUNCH_DOMAIN" "$PLIST"
    STATUS_CMD="launchctl print $LAUNCH_DOMAIN/$PLIST_LABEL"
    LOG_CMD="tail -f $LOG_DIR/packrat.log"
    ;;
esac

# ------------------------------------------------------------------ finished

lan_ip() {
  case "$OS" in
    Darwin) ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null ;;
    *) hostname -I 2>/dev/null | awk '{print $1}' ;;
  esac
}
IP="$(lan_ip || true)"

say ""
if [ "$DRY_RUN" -eq 1 ]; then
  say "Dry run — nothing was changed."
  exit 0
fi
say "Packrat is installed and will start at boot."
say ""
say "  on this machine   http://localhost:$PORT"
[ -n "$IP" ] && say "  on your network   http://$IP:$PORT"
say ""
say "  status            $STATUS_CMD"
say "  logs              $LOG_CMD"
say "  remove            $0 --uninstall"
say ""
if [ -z "$PUBLIC_URL" ] && [ -n "$IP" ]; then
  say "QR codes will point at the address Packrat detects at startup. If this"
  say "machine's IP changes, set it once under Settings, or reinstall with"
  say "  --public-url http://$IP:$PORT"
fi
