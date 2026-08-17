#!/usr/bin/env bash
#
# Reproducible PSS RAM benchmark for OSA.
#
# Measures Proportional Set Size (PSS) of the osagent daemon under three
# conditions, using an isolated data dir and a local mock provider:
#   1. idle                 - daemon up, zero sessions touched
#   2. +10 sessions         - 10 sessions created via the HTTP API
#   3. active conversation  - one message streaming through the mock provider
#
# PSS is read from /proc/<pid>/smaps_rollup (falls back to VmRSS).
# Competitor numbers in BENCHMARKS.md come from the cited sources; run this
# script on your own hardware to reproduce the OSA column.
#
# Usage: ./ram-bench.sh [--iterations N] [--sessions N] [--skip-build]

set -euo pipefail

ITERATIONS=3
SESSION_COUNT=10
SKIP_BUILD=0
BIN="${OSAGENT_BIN:-target/release/osagent}"
WEB_PORT=18765
MOCK_PORT=18766
WORKSPACE_DIR=""
DAEMON_PID=""
MOCK_PID=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="$(mktemp -d /tmp/osagent-rambench.XXXXXX)"

cleanup() {
    [[ -n "$DAEMON_PID" ]] && kill "$DAEMON_PID" 2>/dev/null || true
    [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null || true
    [[ -n "$WORKSPACE_DIR" ]] && rm -rf "$WORKSPACE_DIR" || true
    rm -rf "$DATA_DIR"
}
trap cleanup EXIT

for arg in "$@"; do
    case "$arg" in
        --iterations=*) ITERATIONS="${arg#*=}" ;;
        --sessions=*) SESSION_COUNT="${arg#*=}" ;;
        --skip-build) SKIP_BUILD=1 ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

if [[ "$SKIP_BUILD" != "1" && ! -x "$BIN" ]]; then
    echo ">> building release binary..."
    cargo build --release --bin osagent
fi
[[ -x "$BIN" ]] || { echo "binary not found: $BIN (build it or pass OSAGENT_BIN=...)" >&2; exit 1; }

WORKSPACE_DIR="$DATA_DIR/workspace"
mkdir -p "$WORKSPACE_DIR"

cat > "$DATA_DIR/config.toml" <<EOF
[server]
bind = "127.0.0.1"
port = $WEB_PORT
password_enabled = false
jwt_secret = "ram-bench-secret-do-not-use-in-prod"

[[providers]]
provider_type = "openai"
api_key = "bench-key"
base_url = "http://127.0.0.1:$MOCK_PORT/v1"
model = "mock-gpt"

[agent]
workspace = "$WORKSPACE_DIR"
active_workspace = "default"
workspaces = [
  { id = "default", name = "Default Workspace", path = "$WORKSPACE_DIR", description = "", created_at = "2026-01-01T00:00:00Z", last_used = "2026-01-01T00:00:00Z" }
]

[search]
enabled = false

[storage]
database = "$DATA_DIR/osagent.db"

[logging]
level = "warn"
EOF

echo ">> starting mock provider on :$MOCK_PORT"
python3 "$SCRIPT_DIR/ram-bench/mock_provider.py" --port "$MOCK_PORT" > "$DATA_DIR/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do
    grep -q MOCK_READY "$DATA_DIR/mock.log" && break
    sleep 0.1
done

echo ">> starting osagent (config in $DATA_DIR/config.toml)"
"$BIN" start --config "$DATA_DIR/config.toml" > "$DATA_DIR/daemon.log" 2>&1 &
DAEMON_PID=$!

BASE_URL="http://127.0.0.1:$WEB_PORT"
for _ in $(seq 1 100); do
    if curl -fsS "$BASE_URL/api/auth/status" >/dev/null 2>&1; then break; fi
    sleep 0.2
done
curl -fsS "$BASE_URL/api/auth/status" >/dev/null || { echo "daemon failed to start" >&2; tail -20 "$DATA_DIR/daemon.log" >&2; exit 1; }

TOKEN="$(curl -fsS -X POST "$BASE_URL/api/auth/login" -H 'Content-Type: application/json' \
    -d '{"password":""}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
AUTH="Authorization: Bearer $TOKEN"

pss_mb() {
    local pid="$1"
    if [[ -r "/proc/$pid/smaps_rollup" ]]; then
        awk '/^Pss:/{s+=$2} END {printf "%.1f", s/1024}' "/proc/$pid/smaps_rollup"
    else
        awk '/VmRSS:/{printf "%.1f", $2/1024}' "/proc/$pid/status"
    fi
}

echo ">> warming up (lazy init: providers, tools, frontend)..."
for _ in $(seq 1 3); do
    curl -fsS "$BASE_URL/api/providers" -H "$AUTH" > /dev/null
    curl -fsS "$BASE_URL/api/tools" -H "$AUTH" > /dev/null
    curl -fsS "$BASE_URL/api/sessions" -H "$AUTH" > /dev/null
    sleep 1
done

echo ">> waiting for PSS to stabilize..."
STABLE=0
for _ in $(seq 1 40); do
    WIN=()
    for _ in $(seq 1 5); do
        WIN+=("$(pss_mb "$DAEMON_PID")")
        sleep 1
    done
    SPREAD="$(python3 -c "
import sys
a=[float(x) for x in sys.argv[1:]]
print(f'{max(a)-min(a):.1f}')" "${WIN[@]}")"
    if python3 -c "exit(0 if float('$SPREAD') < 1.0 else 1)"; then
        STABLE=1
        echo "   stable at $(printf '%s ' "${WIN[@]}" | awk '{s+=$1;n++} END {printf "%.1f MB", s/n}')"
        break
    fi
done
[[ "$STABLE" == "1" ]] || { echo "   WARNING: PSS never fully stabilized (spread ${SPREAD} MB); continuing anyway" >&2; }

echo ">> discard conversation (first run lazily loads ~10 MB of tool/provider state)..."
DISCARD_SID="$(curl -fsS "$BASE_URL/api/sessions" -H "$AUTH" -H 'Content-Type: application/json' \
    -d '{"model":"mock-gpt","provider":"openai"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
curl -fsS -X POST "$BASE_URL/api/sessions/$DISCARD_SID/send" -H "$AUTH" \
    -H 'Content-Type: application/json' -d "{\"session_id\":\"$DISCARD_SID\",\"message\":\"discard warmup run\"}" > /dev/null
for _ in $(seq 1 60); do
    curl -fsS "$BASE_URL/api/sessions/$DISCARD_SID" -H "$AUTH" 2>/dev/null \
        | python3 -c 'import json,sys; d=json.load(sys.stdin); n=sum(1 for m in d.get("messages",[]) if m.get("role")=="assistant"); exit(0 if n>=1 else 1)' 2>/dev/null && break
    sleep 0.5
done

echo ">> re-stabilizing after discard run..."
for _ in $(seq 1 40); do
    WIN=()
    for _ in $(seq 1 5); do
        WIN+=("$(pss_mb "$DAEMON_PID")")
        sleep 1
    done
    SPREAD="$(python3 -c "
import sys
a=[float(x) for x in sys.argv[1:]]
print(f'{max(a)-min(a):.1f}')" "${WIN[@]}")"
    if python3 -c "exit(0 if float('$SPREAD') < 1.0 else 1)"; then
        echo "   stable at $(printf '%s ' "${WIN[@]}" | awk '{s+=$1;n++} END {printf "%.1f MB", s/n}')"
        break
    fi
done

sample_pss() {
    # wait for the allocator to release transient request buffers
    # (observed as a PSS dip ~2s after a burst of API calls)
    sleep 3
    local -a samples=()
    local i v
    for i in $(seq 1 5); do
        v="$(pss_mb "$DAEMON_PID")"
        samples+=("$v")
        sleep 1
    done
    printf '%s\n' "${samples[@]}"
}

mean() {
    python3 -c 'import sys; a=[float(x) for x in sys.stdin]; a.sort(); print(f"{a[len(a)//2]:.1f}")'
}

create_sessions() {
    local n="$1"
    for _ in $(seq 1 "$n"); do
        curl -fsS -X POST "$BASE_URL/api/sessions" \
            -H "$AUTH" -H 'Content-Type: application/json' \
            -d '{"model":"mock-gpt","provider":"openai"}' > /dev/null
    done
}

echo ""
echo "================ OSA RAM BENCHMARK ================"
echo "binary:   $BIN"
echo "iterations: $ITERATIONS | sessions: $SESSION_COUNT | pss via smaps_rollup"
echo ""

IDLE_SAMPLES=()
SESS_SAMPLES=()
ACTIVE_PEAKS=()

for iter in $(seq 1 "$ITERATIONS"); do
    echo ">> iteration $iter/$ITERATIONS"
    echo "   phase 1: idle..."
    IDLE_SAMPLES+=("$(sample_pss | mean)")

    echo "   phase 2: creating $SESSION_COUNT sessions..."
    create_sessions "$SESSION_COUNT"
    sleep 2
    SESS_SAMPLES+=("$(sample_pss | mean)")

    echo "   phase 3: active conversation (mock provider streams ~1s)..."
    SID="$(curl -fsS "$BASE_URL/api/sessions" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"model":"mock-gpt","provider":"openai"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
    curl -fsS -X POST "$BASE_URL/api/sessions/$SID/send" -H "$AUTH" \
        -H 'Content-Type: application/json' -d "{\"session_id\":\"$SID\",\"message\":\"run a benchmark conversation\"}" > /dev/null

    PEAK=0
    DONE=0
    for _ in $(seq 1 150); do
        v="$(pss_mb "$DAEMON_PID")"
        PEAK="$(python3 -c "print(max($PEAK, $v))")"
        sleep 0.2
        ASSISTANT="$(curl -fsS "$BASE_URL/api/sessions/$SID" -H "$AUTH" 2>/dev/null \
            | python3 -c 'import json,sys
d=json.load(sys.stdin)
n=sum(1 for m in d.get("messages",[]) if m.get("role")=="assistant")
print(n)' 2>/dev/null || echo 0)"
        if [[ "$ASSISTANT" -ge 1 ]]; then DONE=1; break; fi
    done
    if [[ "$DONE" != "1" ]]; then
        echo "   WARNING: assistant reply not observed; run may have failed" >&2
    fi
    ACTIVE_PEAKS+=("$PEAK")
    echo "   peak during active run: ${PEAK} MB"
done

IDLE_MEAN="$(printf '%s\n' "${IDLE_SAMPLES[@]}" | mean)"
SESS_MEAN="$(printf '%s\n' "${SESS_SAMPLES[@]}" | mean)"
ACTIVE_MEAN="$(printf '%s\n' "${ACTIVE_PEAKS[@]}" | mean)"

echo ""
echo "================ RESULTS (PSS, MB) ================"
echo "idle daemon (0 sessions):          $IDLE_MEAN"
echo "daemon + $SESSION_COUNT idle sessions: $SESS_MEAN"
echo "peak during 1 active conversation: $ACTIVE_MEAN"
echo ""
cat <<EOF
| Condition | PSS (MB) |
|-----------|----------|
| Idle daemon, 0 sessions | $IDLE_MEAN |
| + $SESSION_COUNT sessions created (disk-backed, not resident) | $SESS_MEAN |
| Peak, 1 active conversation | $ACTIVE_MEAN |
EOF
echo ""
echo "raw samples — idle: ${IDLE_SAMPLES[*]} | sessions: ${SESS_SAMPLES[*]} | active peaks: ${ACTIVE_PEAKS[*]}"