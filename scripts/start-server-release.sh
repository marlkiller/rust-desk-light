#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
IP="${RDL_IP:-0.0.0.0}"
PORT="${RDL_PORT:-21115}"
LOG_DIR="${RDL_LOG_DIR:-$ROOT_DIR/target/rdl-server}"
PID_FILE="$LOG_DIR/rdl-server.pid"
LOG_FILE="$LOG_DIR/rdl-server.log"

mkdir -p "$LOG_DIR"

cd "$ROOT_DIR"

if [ "${RDL_SKIP_PULL:-0}" != "1" ]; then
  echo "Updating repository"
  git pull --ff-only
else
  echo "Skipping git pull because RDL_SKIP_PULL=1"
fi

echo "Building rdl-server (release)"
cargo build -p rust-desk-light-server --release
SERVER_BIN="$ROOT_DIR/target/release/rdl-server"

stop_existing() {
  if [ ! -f "$PID_FILE" ]; then
    return 0
  fi

  old_pid="$(cat "$PID_FILE" 2>/dev/null || true)"
  if [ -z "$old_pid" ]; then
    rm -f "$PID_FILE"
    return 0
  fi

  if ! kill -0 "$old_pid" 2>/dev/null; then
    rm -f "$PID_FILE"
    return 0
  fi

  echo "Stopping existing rdl-server pid=$old_pid"
  kill "$old_pid" 2>/dev/null || true

  count=0
  while kill -0 "$old_pid" 2>/dev/null; do
    count=$((count + 1))
    if [ "$count" -ge 30 ]; then
      echo "Existing server did not stop after 30 seconds; sending SIGKILL"
      kill -9 "$old_pid" 2>/dev/null || true
      break
    fi
    sleep 1
  done

  rm -f "$PID_FILE"
}

stop_existing

echo "Starting rdl-server on $IP:$PORT"
nohup "$SERVER_BIN" --ip "$IP" --port "$PORT" >>"$LOG_FILE" 2>&1 &
new_pid="$!"
echo "$new_pid" >"$PID_FILE"

sleep 1
if kill -0 "$new_pid" 2>/dev/null; then
  echo "rdl-server started pid=$new_pid"
  echo "log: $LOG_FILE"
else
  echo "rdl-server failed to start. Last log lines:"
  tail -n 40 "$LOG_FILE" 2>/dev/null || true
  exit 1
fi
