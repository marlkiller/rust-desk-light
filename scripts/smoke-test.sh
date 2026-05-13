#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${RDL_PORT:-21116}"
IP="${RDL_IP:-127.0.0.1}"
LOG_DIR="$ROOT_DIR/target/rdl-smoke"

SERVER_PID=""
CLIENT_PID=""

cleanup() {
  if [[ -n "$CLIENT_PID" ]] && kill -0 "$CLIENT_PID" 2>/dev/null; then
    kill "$CLIENT_PID" 2>/dev/null || true
  fi
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
  fi
}

wait_for_log() {
  local file="$1"
  local pattern="$2"
  local label="$3"

  for _ in $(seq 1 80); do
    if [[ -f "$file" ]] && grep -q "$pattern" "$file"; then
      return 0
    fi
    sleep 0.1
  done

  echo "Timed out waiting for $label"
  echo "--- $file ---"
  [[ -f "$file" ]] && cat "$file"
  exit 1
}

trap cleanup EXIT

mkdir -p "$LOG_DIR"
rm -f "$LOG_DIR"/server.log "$LOG_DIR"/client.log "$LOG_DIR"/admin.log

cd "$ROOT_DIR"

echo "[1/5] Building workspace"
cargo build --workspace

echo "[2/5] Starting server on $IP:$PORT"
"$ROOT_DIR/target/debug/rdl-server" --ip "$IP" --port "$PORT" >"$LOG_DIR/server.log" 2>&1 &
SERVER_PID="$!"
wait_for_log "$LOG_DIR/server.log" "server listening" "server startup"

echo "[3/5] Starting client"
RDL_FORCE_TERMINAL=1 "$ROOT_DIR/target/debug/rdl-client" --ip "$IP" --port "$PORT" >"$LOG_DIR/client.log" 2>&1 &
CLIENT_PID="$!"
wait_for_log "$LOG_DIR/client.log" "client id:" "client registration"

CLIENT_ID="$(sed -n 's/^client id: //p' "$LOG_DIR/client.log" | tail -n 1)"
if [[ -z "$CLIENT_ID" ]]; then
  echo "Could not detect client id"
  cat "$LOG_DIR/client.log"
  exit 1
fi

echo "[4/5] Running admin command flow for client: $CLIENT_ID"
{
  printf 'list\n'
  sleep 0.6
  printf 'cmd %s computer_info\n' "$CLIENT_ID"
  sleep 0.8
  printf 'quit\n'
} | RDL_FORCE_TERMINAL=1 "$ROOT_DIR/target/debug/rdl-admin" --ip "$IP" --port "$PORT" >"$LOG_DIR/admin.log" 2>&1

echo "[5/5] Verifying output"
grep -q "online clients: 1" "$LOG_DIR/admin.log"
grep -q "command=computer_info" "$LOG_DIR/admin.log"
grep -q "hostname=" "$LOG_DIR/admin.log"

echo "Smoke test passed."
echo "Logs: $LOG_DIR"

