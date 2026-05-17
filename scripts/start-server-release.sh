#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
IP="${RDL_IP:-0.0.0.0}"
PORT="${RDL_PORT:-5169}"
LOG_DIR="${RDL_LOG_DIR:-$ROOT_DIR/target/rdl-server-cli}"
PID_FILE="$LOG_DIR/rdl-server-cli.pid"
LOG_FILE="$LOG_DIR/rdl-server-cli.log"

. "$ROOT_DIR/scripts/geoip-db.sh"

mkdir -p "$LOG_DIR"
if [ -f "$LOG_FILE" ]; then
  LOG_START_LINE="$(wc -l <"$LOG_FILE" | tr -d ' ' || echo 0)"
else
  LOG_START_LINE=0
fi

cd "$ROOT_DIR"

if [ "${RDL_SKIP_PULL:-0}" != "1" ]; then
  echo "Updating repository"
  git pull --ff-only
else
  echo "Skipping git pull because RDL_SKIP_PULL=1"
fi

echo "Building rdl-server-cli (release)"
cargo build -p rust-desk-light-server --release
SERVER_BIN="$ROOT_DIR/target/release/rdl-server-cli"
GEOIP_DB_PATH="$(rdl_find_geoip_db "$ROOT_DIR" || true)"

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

  echo "Stopping existing rdl-server-cli pid=$old_pid"
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

if [ -n "$GEOIP_DB_PATH" ]; then
  echo "Using GeoIP database: $GEOIP_DB_PATH"
  echo "Starting rdl-server-cli on $IP:$PORT"
  nohup "$SERVER_BIN" --ip "$IP" --port "$PORT" --geoip-db "$GEOIP_DB_PATH" >>"$LOG_FILE" 2>&1 &
else
  echo "No GeoIP database found; starting without client map locations"
  echo "Starting rdl-server-cli on $IP:$PORT"
  nohup "$SERVER_BIN" --ip "$IP" --port "$PORT" >>"$LOG_FILE" 2>&1 &
fi
new_pid="$!"
echo "$new_pid" >"$PID_FILE"

sleep 1
if kill -0 "$new_pid" 2>/dev/null; then
  echo "rdl-server-cli started pid=$new_pid"
  if [ -n "${RDL_AUTH_TOKEN:-}" ]; then
    echo "auth token: $RDL_AUTH_TOKEN"
  else
    GENERATED_TOKEN="$(
      tail -n +"$((LOG_START_LINE + 1))" "$LOG_FILE" 2>/dev/null \
        | sed -n 's/^generated auth token: //p' \
        | tail -n 1
    )"
    if [ -n "$GENERATED_TOKEN" ]; then
      echo "auth token: $GENERATED_TOKEN"
    else
      echo "auth token: configured by server config or already written to log"
    fi
  fi
  echo "log: $LOG_FILE"
else
  echo "rdl-server-cli failed to start. Last log lines:"
  tail -n 40 "$LOG_FILE" 2>/dev/null || true
  exit 1
fi
