#!/usr/bin/env bash
# Health Push (ADR-0005, issue #33) - push-based survival check.
#
# A host cron (NOT a container) fires this script at a sensible interval. It
# checks the two failure classes a pull model cannot catch:
#   1. Brain Replica lag  - Litestream replication stopped or erroring (silent
#      drift: the user only discovers a stale Brain Replica by opening the admin tab,
#      by which point the ADR-0002 trust contract is already broken).
#   2. Brain File volume  - sqlite_data nearing capacity (about to fill).
# On a breach it pushes to the ntfy.sh webhook so the alert finds the user, not
# the reverse. Zero-RAM, zero-containers between invocations: cron spawns the
# script, it exits. The script ships no secrets - NTFY_WEBHOOK_URL is read from
# infrastructure/.env (ADR-0004); nothing is baked in.
#
# Lag detection (Litestream v0.5.x exposes no lag-seconds gauge, so lag is
# detected structurally): the metrics endpoint at 127.0.0.1:9090 (bound
# loopback-only by the litestream sidecar, #32) being UNREACHABLE means
# Litestream is not running = replication stopped = lag = infinity (the primary
# signal; covers "stop Litestream -> push"). When reachable, an increasing
# litestream_sync_error_count means syncs are being attempted but failing
# (replication erroring = silent drift). A small state file carries the last
# sync-error count + alert timestamps across cron runs so we (a) only alert on
# NEW error growth and (b) don't spam ntfy every tick during a sustained breach
# (cooldown). Recovery from an outage re-baselines the error count so the catch-
# up burst doesn't fire a false alert.
#
# Self-test:  bash infrastructure/health-push.sh --self-test
#   (mocks metrics/volume/ntfy - no docker, no curl, no real webhook needed)
set -euo pipefail

# --- config (env-overridable; prod reads NTFY_WEBHOOK_URL from .env) ----------
# Resolved by config_init() at entry to main()/self_test() so test overrides
# (SB_STATE_DIR, SB_ENV_FILE, ...) take effect even when set after load.
config_init() {
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  ENV_FILE="${SB_ENV_FILE:-$SCRIPT_DIR/.env}"
  LITESTREAM_METRICS_URL="${LITESTREAM_METRICS_URL:-http://127.0.0.1:9090/metrics}"
  SQLITE_VOLUME="${SQLITE_VOLUME:-sqlite_data}"
  VOLUME_CAP_PCT="${VOLUME_CAP_PCT:-85}"
  ALERT_COOLDOWN_SECS="${ALERT_COOLDOWN_SECS:-3600}"
  STATE_DIR="${SB_STATE_DIR:-/var/lib/second-brain}"
  STATE_FILE="$STATE_DIR/health-push.state"
}
config_init   # resolve defaults once (re-resolved by main/self_test as needed)

# --- load NTFY_WEBHOOK_URL from .env (least privilege: only the ntfy key) -----
load_ntfy_url() {
  [[ -z "${NTFY_WEBHOOK_URL:-}" ]] || return 0
  [[ -f "$ENV_FILE" ]] || return 0
  local val
  val="$(grep -E "^NTFY_WEBHOOK_URL=" "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2- || true)"
  val="${val#\"}"; val="${val%\"}"; val="${val#\'}"; val="${val%\'}"
  NTFY_WEBHOOK_URL="$(printf '%s' "$val" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
}

# --- seams (mocked in --self-test via SB_MOCK_* env) -------------------------
fetch_metrics() {
  if [[ -n "${SB_MOCK_METRICS:-}" ]]; then
    [[ "$SB_MOCK_METRICS" == "UNREACHABLE" ]] && return 1
    cat "$SB_MOCK_METRICS"            # canned metrics file
    return 0
  fi
  curl -fsS --max-time 5 "$LITESTREAM_METRICS_URL"
}

# Max litestream_sync_error_count across db labels (metrics on stdin). Prometheus
# metrics carry labels: `litestream_sync_error_count{db="..."} <v>` - the char
# after the name is `{`, not whitespace, so match either form and read $2.
parse_sync_error_count() {
  awk '/^litestream_sync_error_count[{[:space:]]/ {v=$2+0; if (v>m) m=v} END{print m+0}'
}

volume_usage_pct() {
  if [[ -n "${SB_MOCK_VOLUME_PCT:-}" ]]; then printf '%s\n' "$SB_MOCK_VOLUME_PCT"; return 0; fi
  local mp
  mp="$(docker volume inspect -f '{{ .Mountpoint }}' "$SQLITE_VOLUME" 2>/dev/null || true)"
  [[ -n "$mp" ]] || return 1
  df -B1 "$mp" 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5); print $5+0}'
}

push_ntfy() {
  local title="$1" body="$2"
  if [[ -n "${SB_MOCK_NTFY_FILE:-}" ]]; then
    printf '[%s] %s | %s\n' "$(date -u +%FT%TZ)" "$title" "$body" >> "$SB_MOCK_NTFY_FILE"
    return 0
  fi
  # cron redirects stdout+stderr to /dev/null, so a failed push would be silent.
  # Surface it to syslog so the operator can find it: journalctl -t second-brain-health-push
  if [[ -z "${NTFY_WEBHOOK_URL:-}" ]]; then
    echo "health-push: NTFY_WEBHOOK_URL unset - cannot push alert: $body" >&2
    logger -t second-brain-health-push "NTFY_WEBHOOK_URL unset - alert NOT pushed: $body" 2>/dev/null || true
    return 1
  fi
  if ! curl -fsS --max-time 10 \
      -H "Title: $title" -H "Tags: warning" -H "Priority: high" \
      -d "$body" "$NTFY_WEBHOOK_URL"; then
    logger -t second-brain-health-push "ntfy push failed - alert NOT delivered: $body" 2>/dev/null || true
    return 1
  fi
}

# --- state (carries last error count + alert timestamps across cron runs) -----
state_load() {
  LAST_ALERT_DOWN=0; LAST_ALERT_ERR=0; LAST_ALERT_VOL=0; LAST_SYNC_ERR=0; WAS_DOWN=0
  [[ -f "$STATE_FILE" ]] || return 0
  while IFS='=' read -r k v; do
    case "$k" in
      last_alert_down)        LAST_ALERT_DOWN="${v:-0}";;
      last_alert_err)         LAST_ALERT_ERR="${v:-0}";;
      last_alert_volume)      LAST_ALERT_VOL="${v:-0}";;
      last_sync_error_count)  LAST_SYNC_ERR="${v:-0}";;
      was_down)               WAS_DOWN="${v:-0}";;
    esac
  done < "$STATE_FILE"
}

state_save() {
  mkdir -p "$STATE_DIR"
  cat > "$STATE_FILE" <<EOF
last_alert_down=$LAST_ALERT_DOWN
last_alert_err=$LAST_ALERT_ERR
last_alert_volume=$LAST_ALERT_VOL
last_sync_error_count=$LAST_SYNC_ERR
was_down=$WAS_DOWN
EOF
}

# --- core check (uses globals set by state_load; appends to messages[]) ------
run_check() {
  local now metrics cur_err pct
  messages=()
  now=$(date +%s)

  # 1. Brain Replica lag / replication health.
  if metrics="$(fetch_metrics)"; then
    cur_err="$(printf '%s' "$metrics" | parse_sync_error_count)"
    if [[ "$WAS_DOWN" -eq 1 ]]; then
      # Just recovered: re-baseline so the outage's catch-up error burst does
      # not fire a false "sync errors increased" alert. No alert on recovery.
      WAS_DOWN=0
      LAST_SYNC_ERR="$cur_err"
    elif [[ "$cur_err" -gt "$LAST_SYNC_ERR" ]]; then
      if (( now - LAST_ALERT_ERR >= ALERT_COOLDOWN_SECS )); then
        messages+=("Litestream sync errors increased: $LAST_SYNC_ERR -> $cur_err - replication failing (silent drift)")
        LAST_ALERT_ERR="$now"
      fi
      LAST_SYNC_ERR="$cur_err"
    else
      LAST_SYNC_ERR="$cur_err"
    fi
  else
    # Metrics endpoint unreachable => Litestream not running => replication
    # stopped => lag = infinity. This is the primary survival signal.
    WAS_DOWN=1
    if (( now - LAST_ALERT_DOWN >= ALERT_COOLDOWN_SECS )); then
      messages+=("Litestream replication DOWN - metrics endpoint $LITESTREAM_METRICS_URL unreachable; the Brain Replica is NOT being updated")
      LAST_ALERT_DOWN="$now"
    fi
  fi

  # 2. Brain File volume capacity.
  pct="$(volume_usage_pct 2>/dev/null || true)"
  if [[ "$pct" =~ ^[0-9]+$ ]] && (( pct > VOLUME_CAP_PCT )); then
    if (( now - LAST_ALERT_VOL >= ALERT_COOLDOWN_SECS )); then
      messages+=("Brain File volume $SQLITE_VOLUME at ${pct}% (threshold ${VOLUME_CAP_PCT}%) - about to fill")
      LAST_ALERT_VOL="$now"
    fi
  fi
}

main() {
  config_init
  load_ntfy_url
  state_load
  run_check
  if (( ${#messages[@]} > 0 )); then
    push_ntfy "Second Brain alert" "$(printf '%s\n' "${messages[@]}")"
  fi
  state_save
}

# --- self-test (RED/GREEN without docker, curl, or a real webhook) -----------
self_test() {
  local tmp fail=0
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  export SB_STATE_DIR="$tmp/state" SB_MOCK_NTFY_FILE="$tmp/ntfy.log"
  export SB_ENV_FILE="$tmp/no-such-env"   # isolate from the real .env
  export VOLUME_CAP_PCT=85 ALERT_COOLDOWN_SECS=3600 SQLITE_VOLUME=sqlite_data
  export LITESTREAM_METRICS_URL=http://127.0.0.1:9090/metrics
  config_init   # pick up the exported overrides above
  mkdir -p "$SB_STATE_DIR"
  touch "$SB_MOCK_NTFY_FILE"

  # Canned metrics fixtures: a healthy db (no sync errors) and an erroring one.
  local metrics_ok="$tmp/metrics_ok" metrics_err5="$tmp/metrics_err5" metrics_err9="$tmp/metrics_err9"
  printf 'litestream_sync_count{db="/data/second_brain.db"} 10\nlitestream_sync_error_count{db="/data/second_brain.db"} 0\n' > "$metrics_ok"
  printf 'litestream_sync_count{db="/data/second_brain.db"} 11\nlitestream_sync_error_count{db="/data/second_brain.db"} 5\n' > "$metrics_err5"
  printf 'litestream_sync_count{db="/data/second_brain.db"} 12\nlitestream_sync_error_count{db="/data/second_brain.db"} 9\n' > "$metrics_err9"

  # check <label> <want-alerts>: count + truncate the mock ntfy log.
  check() { local label="$1" want="$2" got; got="$(wc -l < "$SB_MOCK_NTFY_FILE" | tr -d ' ')"; : > "$SB_MOCK_NTFY_FILE";
    if [[ "$got" == "$want" ]]; then printf 'ok   - %s (alerts=%s)\n' "$label" "$got";
    else printf 'FAIL - %s: expected %s alert(s), got %s\n' "$label" "$want" "$got" >&2; fail=1; fi; }

  # 1. Green: reachable, no errors, volume low -> no alert.
  SB_MOCK_METRICS="$metrics_ok"  SB_MOCK_VOLUME_PCT=20 main; check "healthy: no alert" 0

  # 2. RED: Litestream stopped (metrics unreachable) -> 1 alert.
  SB_MOCK_METRICS=UNREACHABLE    SB_MOCK_VOLUME_PCT=20 main; check "litestream down -> 1 alert" 1

  # 3. Cooldown: still down, within cooldown -> no second alert.
  SB_MOCK_METRICS=UNREACHABLE    SB_MOCK_VOLUME_PCT=20 main; check "still down within cooldown -> no re-alert" 0

  # 4. Recovery: reachable again with a higher error count (the outage catch-up
  #    burst) -> NO false "sync errors increased" alert (re-baselined).
  SB_MOCK_METRICS="$metrics_err5" SB_MOCK_VOLUME_PCT=20 main; check "recovery re-baselines error burst -> no false alert" 0

  # 5. A FURTHER error increase after recovery (real new errors) -> 1 alert.
  SB_MOCK_METRICS="$metrics_err9" SB_MOCK_VOLUME_PCT=20 main; check "new sync errors after recovery -> 1 alert" 1

  # 6. Volume breach -> 1 alert (independent cooldown from the lag alerts).
  SB_MOCK_METRICS="$metrics_ok"  SB_MOCK_VOLUME_PCT=95 main; check "volume near capacity -> 1 alert" 1

  # 7. Volume cooldown: still high, within cooldown -> no re-alert.
  SB_MOCK_METRICS="$metrics_ok"  SB_MOCK_VOLUME_PCT=95 main; check "volume still high within cooldown -> no re-alert" 0

  # State file persisted the expected keys across runs.
  if grep -qE '^(last_alert_down|last_alert_err|last_alert_volume|last_sync_error_count|was_down)=' "$SB_STATE_DIR/health-push.state"; then
    printf 'ok   - state file carries keys across runs\n'
  else
    printf 'FAIL - state file missing expected keys\n' >&2; fail=1
  fi

  # No-secret-in-script guard (ADR-0004): the script must not bake a literal
  # ntfy topic URL or auth token - only the $NTFY_WEBHOOK_URL variable. Patterns
  # match a real leaked value (ntfy.sh/<topic> or auth=<token>); the script's
  # own comments say "ntfy.sh webhook" (no slash-topic) so they do not match,
  # and the backslashed pattern below does not match itself either.
  if grep -En 'ntfy\.sh/[A-Za-z0-9_-]{4,}|auth=[A-Za-z0-9_-]{16,}' "${BASH_SOURCE[0]}"; then
    printf 'FAIL - script bakes a literal ntfy topic URL / auth token (ADR-0004)\n' >&2; fail=1
  else
    printf 'ok   - script ships no ntfy secret literal (ADR-0004)\n'
  fi

  if [[ "$fail" -ne 0 ]]; then
    echo "FAIL - health-push self-test failed" >&2
    exit 1
  fi
  echo
  echo "health-push self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit 0
fi

main
