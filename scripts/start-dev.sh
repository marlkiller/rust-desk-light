#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IP="${RDL_IP:-127.0.0.1}"
PORT="${RDL_PORT:-5169}"
LOG_DIR="$ROOT_DIR/target/rdl-dev"

mkdir -p "$LOG_DIR"

echo "Building rust-desk-light"
cargo build --workspace --manifest-path "$ROOT_DIR/Cargo.toml"

SERVER_CMD="cd '$ROOT_DIR' && ./target/debug/rdl-server --ip '$IP' --port '$PORT'"
CLIENT_BIN="$ROOT_DIR/target/debug/rdl-client"
ADMIN_BIN="$ROOT_DIR/target/debug/rdl-admin"

echo "Starting rust-desk-light dev stack"
echo "server: $IP:$PORT"
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
    "$CLIENT_BIN" --ip "$IP" --port "$PORT" >"$LOG_DIR/client.log" 2>&1 &
    sleep 1
    "$ADMIN_BIN" --ip "$IP" --port "$PORT" >"$LOG_DIR/admin.log" 2>&1 &
    ;;
  Linux)
    if command -v gnome-terminal >/dev/null 2>&1; then
      gnome-terminal --title="rdl-server" -- bash -lc "$SERVER_CMD; exec bash"
    elif command -v konsole >/dev/null 2>&1; then
      konsole --new-tab -p tabtitle="rdl-server" -e bash -lc "$SERVER_CMD; exec bash"
    elif command -v xterm >/dev/null 2>&1; then
      xterm -T "rdl-server" -e bash -lc "$SERVER_CMD; exec bash" &
    else
      echo "No supported terminal emulator found."
      echo "Run the server command manually, then start client/admin binaries:"
      echo "  $SERVER_CMD"
      exit 1
    fi
    sleep 1
    "$CLIENT_BIN" --ip "$IP" --port "$PORT" >"$LOG_DIR/client.log" 2>&1 &
    sleep 1
    "$ADMIN_BIN" --ip "$IP" --port "$PORT" >"$LOG_DIR/admin.log" 2>&1 &
    ;;
  *)
    echo "Unsupported platform for this shell launcher."
    echo "Use scripts/start-dev.ps1 on Windows."
    exit 1
    ;;
esac

echo "Started server terminal, client GUI, and admin GUI."
