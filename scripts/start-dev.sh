#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_IP="127.0.0.1"
DEFAULT_PORT="5169"
DEFAULT_AUTH_TOKEN="change-me"

IP="${RDL_IP:-$DEFAULT_IP}"
PORT="${RDL_PORT:-$DEFAULT_PORT}"
AUTH_TOKEN="${RDL_AUTH_TOKEN:-$DEFAULT_AUTH_TOKEN}"
LOG_DIR="$ROOT_DIR/target/rdl-dev"

source "$ROOT_DIR/scripts/geoip-db.sh"

mkdir -p "$LOG_DIR"
: >"$LOG_DIR/server.log"

shell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

echo "Building rust-desk-light"
cargo build --workspace --manifest-path "$ROOT_DIR/Cargo.toml"

GEOIP_DB_PATH="$(rdl_find_geoip_db "$ROOT_DIR" || true)"
SERVER_CMD="cd $(shell_quote "$ROOT_DIR") && ./target/debug/rdl-server-cli --ip $(shell_quote "$IP") --port $(shell_quote "$PORT") --auth-token $(shell_quote "$AUTH_TOKEN")"
if [[ -n "$GEOIP_DB_PATH" ]]; then
  SERVER_CMD="$SERVER_CMD --geoip-db $(shell_quote "$GEOIP_DB_PATH")"
fi
SERVER_CMD="$SERVER_CMD 2>&1 | tee $(shell_quote "$LOG_DIR/server.log")"
CLIENT_BIN="$ROOT_DIR/target/debug/rdl-client-gui"
ADMIN_BIN="$ROOT_DIR/target/debug/rdl-admin-gui"

echo "Starting rust-desk-light dev stack"
echo "server: $IP:$PORT"
echo "auth token: $AUTH_TOKEN"
if [[ -n "$GEOIP_DB_PATH" ]]; then
  echo "geoip: $GEOIP_DB_PATH"
else
  echo "geoip: disabled (no GeoLite2-City db/archive found)"
fi
echo "logs: $LOG_DIR"
echo

case "$(uname -s)" in
  Darwin)
    osascript <<EOF
tell application "Terminal"
  activate
  do script "$SERVER_CMD"
end tell
EOF
    sleep 1
    "$CLIENT_BIN" --ip "$IP" --port "$PORT" --auth-token "$AUTH_TOKEN" >"$LOG_DIR/client.log" 2>&1 &
    sleep 1
    "$ADMIN_BIN" --ip "$IP" --port "$PORT" --auth-token "$AUTH_TOKEN" >"$LOG_DIR/admin.log" 2>&1 &
    ;;
  Linux)
    if command -v gnome-terminal >/dev/null 2>&1; then
      gnome-terminal --title="rdl-server-cli" -- bash -lc "$SERVER_CMD; exec bash"
    elif command -v konsole >/dev/null 2>&1; then
      konsole --new-tab -p tabtitle="rdl-server-cli" -e bash -lc "$SERVER_CMD; exec bash"
    elif command -v xterm >/dev/null 2>&1; then
      xterm -T "rdl-server-cli" -e bash -lc "$SERVER_CMD; exec bash" &
    else
      echo "No supported terminal emulator found."
      echo "Run the server command manually, then start client/admin binaries:"
      echo "  $SERVER_CMD"
      exit 1
    fi
    sleep 1
    "$CLIENT_BIN" --ip "$IP" --port "$PORT" --auth-token "$AUTH_TOKEN" >"$LOG_DIR/client.log" 2>&1 &
    sleep 1
    "$ADMIN_BIN" --ip "$IP" --port "$PORT" --auth-token "$AUTH_TOKEN" >"$LOG_DIR/admin.log" 2>&1 &
    ;;
  *)
    echo "Unsupported platform for this shell launcher."
    echo "Use scripts/start-dev.ps1 on Windows."
    exit 1
    ;;
esac

echo "Started server terminal, client GUI, and admin GUI."
