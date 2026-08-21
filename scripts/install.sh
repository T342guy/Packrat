#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Installs Packrat and registers it to start at boot.
#
#   Linux (systemd) — a sandboxed system service under its own account when run
#                     with sudo, otherwise a user service with lingering so it
#                     still starts at boot without anyone logging in.
#   macOS (launchd) — a LaunchDaemon under sudo, a LaunchAgent without.
#
# Your inventory database is never touched: uninstalling leaves it in place.
#
# shellcheck shell=bash

# Someone will inevitably run this as `sh install.sh`. Re-exec under bash
# rather than failing on the first [[ ]].
if [ -z "${BASH_VERSION:-}" ]; then
  exec bash "$0" "$@"
fi

set -Eeuo pipefail

readonly APP="packrat"
readonly PLIST_LABEL="dev.packrat"
readonly REPO="https://github.com/T342guy/packrat"

# Defaults; every one can be overridden by a flag.
PORT="8080"
HOST="0.0.0.0"
LOG_LEVEL="info"
DB=""
PUBLIC_URL=""
BINARY=""
PREFIX=""
MODE=""
DRY_RUN=0
UNINSTALL=0

# Filled in once the platform and mode are known.
INIT=""
BIN_DIR=""
BIN_PATH=""
DB_DIR=""
UNIT_PATH=""
PLIST_PATH=""
LAUNCH_DOMAIN=""
LOG_DIR=""
STATUS_CMD=""
LOG_CMD=""
declare -a EXEC_ARGS=()

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

on_error() {
  local line=$1
  printf '\nerror: failed at line %s. Nothing further was changed.\n' "$line" >&2
  printf 'Re-run with --dry-run to see what it was about to do.\n' >&2
}
trap 'on_error "$LINENO"' ERR

say()  { printf '%s\n' "$*"; }
step() { printf '  %s\n' "$*"; }

usage() {
  cat <<'USAGE'
Usage: scripts/install.sh [options]

Installs the packrat binary and sets it to start at boot.

Options:
  -p, --port <PORT>       Port to serve on (default: 8080)
      --host <ADDR>       Address to bind (default: 0.0.0.0, reachable on your LAN)
  -d, --db <PATH>         Database file (default: per-platform state directory)
      --public-url <URL>  Base URL for QR codes (default: auto-detected at runtime)
      --log-level <LEVEL> off, error, warn, info, debug, trace (default: info)
      --binary <PATH>     Use this prebuilt binary instead of building one
      --prefix <DIR>      Install root (default: /usr/local as root, ~/.local otherwise)
      --system            Force a system-wide service (needs root)
      --user              Force a per-user service
  -n, --dry-run           Print every file and command, change nothing
      --uninstall         Stop and remove the service; keeps the database
  -h, --help              This message

Examples:
  sudo scripts/install.sh                       # system service, starts at boot
  sudo scripts/install.sh --port 9000 --public-url http://192.168.1.24:9000
  scripts/install.sh --user                     # just for me, no root needed
  scripts/install.sh --dry-run                  # inspect the service file first
  sudo scripts/install.sh --uninstall
USAGE
}

parse_args() {
  while (($# > 0)); do
    case "$1" in
      -p|--port)       PORT="${2:?--port needs a value}"; shift 2 ;;
      --host)          HOST="${2:?--host needs a value}"; shift 2 ;;
      -d|--db)         DB="${2:?--db needs a value}"; shift 2 ;;
      --public-url)    PUBLIC_URL="${2:?--public-url needs a value}"; shift 2 ;;
      --log-level)     LOG_LEVEL="${2:?--log-level needs a value}"; shift 2 ;;
      --binary)        BINARY="${2:?--binary needs a value}"; shift 2 ;;
      --prefix)        PREFIX="${2:?--prefix needs a value}"; shift 2 ;;
      --system)        MODE="system"; shift ;;
      --user)          MODE="user"; shift ;;
      -n|--dry-run)    DRY_RUN=1; shift ;;
      --uninstall)     UNINSTALL=1; shift ;;
      -h|--help)       usage; exit 0 ;;
      *) printf 'unknown option: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
  done

  [[ "$PORT" =~ ^[0-9]+$ ]] || die "--port must be a number, got '$PORT'"
  ((PORT > 0 && PORT < 65536)) || die "--port must be between 1 and 65535"
}

# Every mutating action goes through this, so --dry-run is honest by design.
run() {
  if ((DRY_RUN)); then
    printf '  would run: %s\n' "$*"
  else
    "$@"
  fi
}

# Writes stdin to a file, using sudo when the destination needs it.
write_file() {
  local target="$1"
  if ((DRY_RUN)); then
    printf '\n  would write %s:\n\n' "$target"
    sed 's/^/    /'
    printf '\n'
    return
  fi
  local dir
  dir="$(dirname "$target")"
  if [[ -w "$dir" ]]; then
    cat > "$target"
  else
    sudo tee "$target" > /dev/null
  fi
}

detect_platform() {
  case "$(uname -s)" in
    Linux)
      command -v systemctl > /dev/null 2>&1 ||
        die "this script only knows systemd on Linux.
Run packrat under your own init system, or use the container image:
  docker run -d -p ${PORT}:8080 -v packrat-data:/data ghcr.io/t342guy/packrat:latest"
      INIT="systemd"
      ;;
    Darwin) INIT="launchd" ;;
    *) die "unsupported system: $(uname -s). The container image runs anywhere Docker does." ;;
  esac

  if [[ -z "$MODE" ]]; then
    if (($(id -u) == 0)); then MODE="system"; else MODE="user"; fi
  fi
  if [[ "$MODE" == "system" ]] && (($(id -u) != 0)); then
    die "a system service needs root — re-run with sudo, or pass --user"
  fi
}

resolve_paths() {
  if [[ -z "$PREFIX" ]]; then
    if [[ "$MODE" == "system" ]]; then PREFIX="/usr/local"; else PREFIX="$HOME/.local"; fi
  fi
  BIN_DIR="$PREFIX/bin"
  BIN_PATH="$BIN_DIR/$APP"

  if [[ -z "$DB" ]]; then
    if [[ "$INIT" == "launchd" ]]; then
      if [[ "$MODE" == "system" ]]; then
        DB="/Library/Application Support/Packrat/inventory.db"
      else
        DB="$HOME/Library/Application Support/Packrat/inventory.db"
      fi
    elif [[ "$MODE" == "system" ]]; then
      DB="/var/lib/$APP/inventory.db"
    else
      DB="${XDG_DATA_HOME:-$HOME/.local/share}/$APP/inventory.db"
    fi
  fi
  DB_DIR="$(dirname "$DB")"

  if [[ "$MODE" == "user" ]]; then
    UNIT_PATH="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$APP.service"
  else
    UNIT_PATH="/etc/systemd/system/$APP.service"
  fi

  if [[ "$INIT" == "launchd" && "$MODE" == "system" ]]; then
    # A daemon starts at boot with nobody logged in; an agent needs a login
    # session. For a machine that lives in the garage, that difference matters.
    PLIST_PATH="/Library/LaunchDaemons/$PLIST_LABEL.plist"
    LAUNCH_DOMAIN="system"
    LOG_DIR="/Library/Logs"
  else
    PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"
    LAUNCH_DOMAIN="gui/$(id -u)"
    LOG_DIR="$HOME/Library/Logs"
  fi

  # An array, not a string: the conventional macOS data directory contains a
  # space, and a --db from the command line might too.
  EXEC_ARGS=(--db "$DB" --host "$HOST" --port "$PORT" --log "$LOG_LEVEL")
  if [[ -n "$PUBLIC_URL" ]]; then
    EXEC_ARGS+=(--public-url "$PUBLIC_URL")
  fi
}

# systemd splits on whitespace unless a value is double-quoted.
systemd_quote() {
  local value="$1"
  if [[ "$value" == *[[:space:]]* ]]; then printf '"%s"' "$value"; else printf '%s' "$value"; fi
}

exec_start_line() {
  local line
  line="$(systemd_quote "$BIN_PATH")"
  local arg
  for arg in "${EXEC_ARGS[@]}"; do
    line+=" $(systemd_quote "$arg")"
  done
  printf '%s' "$line"
}

xml_escape() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

find_binary() {
  [[ -n "$BINARY" ]] && return 0

  local here
  here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

  if [[ -x "$here/target/release/$APP" ]]; then
    BINARY="$here/target/release/$APP"
    step "Using the release build already in this checkout."
  elif command -v cargo > /dev/null 2>&1 && [[ -f "$here/Cargo.toml" ]]; then
    step "Building from source (a minute or two)…"
    if ((DRY_RUN)); then
      printf '  would run: cargo build --release --locked (in %s)\n' "$here"
    else
      (cd "$here" && cargo build --release --locked)
    fi
    BINARY="$here/target/release/$APP"
  elif command -v "$APP" > /dev/null 2>&1; then
    BINARY="$(command -v "$APP")"
    step "Using the packrat already on your PATH: $BINARY"
  else
    die "no binary here and no Rust toolchain to build one.
Install Rust from https://rustup.rs and re-run, point --binary at a prebuilt
binary from $REPO/releases, or skip all this and run the container image:
  docker run -d --name packrat --restart unless-stopped \\
    -p ${PORT}:8080 -v packrat-data:/data \\
    -e PACKRAT_PUBLIC_URL=http://<this-machine>:${PORT} \\
    ghcr.io/t342guy/packrat:latest"
  fi
}

install_systemd_system() {
  step "Creating the '$APP' service account."
  if ((DRY_RUN)); then
    printf '  would run: useradd --system --home-dir /var/lib/%s --shell /usr/sbin/nologin %s\n' \
      "$APP" "$APP"
  elif ! id "$APP" > /dev/null 2>&1; then
    useradd --system --home-dir "/var/lib/$APP" --shell /usr/sbin/nologin "$APP" ||
      die "could not create the $APP account"
  fi
  run mkdir -p "$DB_DIR"
  run chown "$APP:$APP" "$DB_DIR"

  step "Writing $UNIT_PATH"
  write_file "$UNIT_PATH" <<UNIT
[Unit]
Description=Packrat inventory
Documentation=$REPO
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$APP
Group=$APP
ExecStart=$(exec_start_line)
Restart=on-failure
RestartSec=5

# Logs go to the journal: journalctl -u $APP
Environment=PACKRAT_LOG_LEVEL=$LOG_LEVEL

# It serves a local web app from one SQLite file and needs nothing else.
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
ReadWritePaths=$(systemd_quote "$DB_DIR")

[Install]
WantedBy=multi-user.target
UNIT

  run systemctl daemon-reload
  run systemctl enable --now "$APP.service"
  STATUS_CMD="systemctl status $APP"
  LOG_CMD="journalctl -u $APP -f"
}

install_systemd_user() {
  run mkdir -p "$DB_DIR" "$(dirname "$UNIT_PATH")"
  step "Writing $UNIT_PATH"
  write_file "$UNIT_PATH" <<UNIT
[Unit]
Description=Packrat inventory
Documentation=$REPO
After=network-online.target

[Service]
Type=simple
ExecStart=$(exec_start_line)
Restart=on-failure
RestartSec=5
Environment=PACKRAT_LOG_LEVEL=$LOG_LEVEL
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=default.target
UNIT

  run systemctl --user daemon-reload
  run systemctl --user enable --now "$APP.service"

  # Without lingering a user service stops at logout and does not return at
  # boot, which defeats the point on a machine in the garage.
  step "Enabling lingering so it survives logout and starts at boot."
  if ((DRY_RUN)); then
    printf '  would run: loginctl enable-linger %s\n' "$(id -un)"
  else
    loginctl enable-linger "$(id -un)" 2> /dev/null ||
      say "  note: lingering needs root; it will start when you log in instead."
  fi
  STATUS_CMD="systemctl --user status $APP"
  LOG_CMD="journalctl --user -u $APP -f"
}

install_launchd() {
  run mkdir -p "$DB_DIR" "$(dirname "$PLIST_PATH")" "$LOG_DIR"

  local args_xml="" arg
  for arg in "${EXEC_ARGS[@]}"; do
    args_xml+="
    <string>$(xml_escape "$arg")</string>"
  done

  # Run as the human who invoked sudo rather than as root.
  local run_as=""
  if [[ "$MODE" == "system" ]]; then
    run_as="${SUDO_USER:-root}"
  fi

  step "Writing $PLIST_PATH"
  write_file "$PLIST_PATH" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$PLIST_LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml_escape "$BIN_PATH")</string>$args_xml
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PACKRAT_LOG_LEVEL</key>
    <string>$LOG_LEVEL</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>${run_as:+
  <key>UserName</key>
  <string>$run_as</string>}
  <key>StandardOutPath</key>
  <string>$LOG_DIR/packrat.log</string>
  <key>StandardErrorPath</key>
  <string>$LOG_DIR/packrat.log</string>
</dict>
</plist>
PLIST

  run launchctl bootout "$LAUNCH_DOMAIN/$PLIST_LABEL" || true
  run launchctl bootstrap "$LAUNCH_DOMAIN" "$PLIST_PATH"
  STATUS_CMD="launchctl print $LAUNCH_DOMAIN/$PLIST_LABEL"
  LOG_CMD="tail -f $LOG_DIR/packrat.log"
}

uninstall() {
  say "Removing the Packrat service ($INIT, $MODE)."
  case "$INIT:$MODE" in
    systemd:system)
      run systemctl stop "$APP.service" || true
      run systemctl disable "$APP.service" || true
      run rm -f "$UNIT_PATH"
      run systemctl daemon-reload
      ;;
    systemd:user)
      run systemctl --user stop "$APP.service" || true
      run systemctl --user disable "$APP.service" || true
      run rm -f "$UNIT_PATH"
      run systemctl --user daemon-reload
      ;;
    launchd:*)
      run launchctl bootout "$LAUNCH_DOMAIN/$PLIST_LABEL" || true
      run rm -f "$PLIST_PATH"
      ;;
  esac
  run rm -f "$BIN_PATH"

  say ""
  say "Done. Your inventory was left untouched at:"
  say "  $DB"
  say "Delete it yourself if you really mean to."
}

lan_address() {
  case "$(uname -s)" in
    Darwin) ipconfig getifaddr en0 2> /dev/null || ipconfig getifaddr en1 2> /dev/null || true ;;
    *) hostname -I 2> /dev/null | awk '{print $1}' || true ;;
  esac
}

report() {
  local ip
  ip="$(lan_address)"

  say ""
  if ((DRY_RUN)); then
    say "Dry run — nothing was changed."
    return
  fi
  say "Packrat is installed and will start at boot."
  say ""
  say "  on this machine   http://localhost:$PORT"
  if [[ -n "$ip" ]]; then
    say "  on your network   http://$ip:$PORT"
  fi
  say ""
  say "  status            $STATUS_CMD"
  say "  logs              $LOG_CMD"
  say "  live logs         $BIN_PATH --hook-logging --port $PORT"
  say "  remove            $0 --uninstall"
  say ""
  if [[ -z "$PUBLIC_URL" && -n "$ip" ]]; then
    say "QR codes point at whatever address Packrat detects when it starts. If this"
    say "machine's IP moves, set it once under Settings, or reinstall with"
    say "  --public-url http://$ip:$PORT"
  fi
}

main() {
  parse_args "$@"
  detect_platform
  resolve_paths

  if ((UNINSTALL)); then
    uninstall
    exit 0
  fi

  say "Packrat installer"
  say "  service    $INIT, $MODE-wide"
  say "  binary     $BIN_PATH"
  say "  database   $DB"
  say "  address    http://$HOST:$PORT"
  say "  logging    $LOG_LEVEL"
  say ""

  find_binary
  step "Installing the binary."
  run mkdir -p "$BIN_DIR"
  run install -m 0755 "$BINARY" "$BIN_PATH"

  case "$INIT:$MODE" in
    systemd:system) install_systemd_system ;;
    systemd:user)   install_systemd_user ;;
    launchd:*)      install_launchd ;;
  esac

  report
}

main "$@"
