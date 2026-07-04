#!/usr/bin/env bash
# Structural (and optional live) tests for the two-service Compose topology
# from issue #29 / ADR-0001 / ADR-0006 / ADR-0007.
#
#   bash infrastructure/test/topology.sh           # structural (fast, no build)
#   bash infrastructure/test/topology.sh --live    # also build, up, curl, down
#
# Structural tests assert the Compose file's shape and the Edge's Zero-Trust
# Dockerfile. Live tests boot the stack and curl the Edge for the PWA Bundle
# and a proxied /api/health. `docker compose config` resolves env_file into
# the backend `environment`, so env_file is asserted against the raw YAML by
# grep (never against resolved values) and no secret is ever printed. Uses
# only Python stdlib (json) + grep — no third-party deps, so it runs in CI.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

LIVE=0
[[ "${1:-}" == "--live" ]] && LIVE=1

pass() { printf 'ok   - %s\n' "$*"; }
die()  { printf 'FAIL - %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }
need docker
need python3

docker compose config >/dev/null
pass "docker compose config parses"

# --- resolved-JSON structural shape (stdlib json only) -----------------------
python3 - "$REPO_ROOT" <<'PY'
import json, subprocess, sys
repo = sys.argv[1]

cfg = json.loads(subprocess.check_output(
    ["docker", "compose", "config", "--format", "json"], cwd=repo, text=True))
svc = cfg["services"]
assert set(svc) == {"edge", "backend"}, f"expected edge+backend, got {set(svc)}"
be, edge = svc["backend"], svc["edge"]

# ADR-0006: Backend internal-only — no published host ports, expose 8080.
assert not be.get("ports"), f"backend must NOT publish ports (ADR-0006); got {be.get('ports')}"
assert any("8080" in str(e) for e in be.get("expose", [])), \
    f"backend must expose 8080; got {be.get('expose')}"

# Brain File on the named volume sqlite_data at /data (never a bind mount).
dvols = [v for v in be.get("volumes", []) if isinstance(v, dict) and v.get("target") == "/data"]
assert dvols, f"backend must mount a volume at /data; got {be.get('volumes')}"
m = dvols[0]
assert m.get("type") == "volume", f"/data must be a named volume; got type={m.get('type')}"
assert m.get("source") == "sqlite_data", f"/data must use sqlite_data; got {m.get('source')}"
assert "sqlite_data" in cfg.get("volumes", {}), "top-level volume sqlite_data must be declared"

# Edge is the sole published port (:80); both services on one internal network.
assert edge.get("ports"), "edge must publish :80"
assert any(p.get("target") == 80 for p in edge["ports"]), f"edge must publish :80; got {edge['ports']}"
assert "app_network" in (be.get("networks") or []), "backend must be on app_network"
assert "app_network" in (edge.get("networks") or []), "edge must be on app_network"
assert "app_network" in cfg.get("networks", {}), "app_network must be declared"

print("ok   - backend internal-only (expose 8080, no ports) per ADR-0006")
print("ok   - Brain File on named volume sqlite_data at /data")
print("ok   - edge sole published :80; both on app_network")
PY

# --- raw YAML by grep: env_file (ADR-0004) + image-tag fallback (#31) --------
grep -qE '^[[:space:]]*env_file:' docker-compose.yml \
  || die "backend must declare an env_file directive (ADR-0004)"
grep -qE '^[[:space:]]*-[[:space:]]+infrastructure/\.env[[:space:]]*$' docker-compose.yml \
  || die "backend env_file must list infrastructure/.env (ADR-0004)"
grep -F 'second-brain-edge:${EDGE_TAG:-latest}' docker-compose.yml >/dev/null \
  || die "edge image must use \${EDGE_TAG:-latest} (ADR-0007 / #31)"
grep -F 'second-brain-backend:${BACKEND_TAG:-latest}' docker-compose.yml >/dev/null \
  || die "backend image must use \${BACKEND_TAG:-latest} (ADR-0007 / #31)"
pass "env_file -> infrastructure/.env (ADR-0004); image tags fall back to latest"

# --- Edge Zero-Trust Dockerfile (ADR-0004) -----------------------------------
DF="$REPO_ROOT/infrastructure/edge/Dockerfile"
[[ -f "$DF" ]] || die "missing $DF"
grep -qE '^FROM node:' "$DF" || die "Edge Dockerfile must have a Node (PWA Bundle) stage"
grep -qE '^FROM caddy:' "$DF" || die "Edge Dockerfile must have a Caddy stage"
grep -qE 'COPY --from=bundle' "$DF" || die "Edge Dockerfile must COPY the PWA Bundle from the build stage"
if grep -iEn '\.env|COPY[^E]*[Ss]ecret|ENV.*[A-Z0-9_]*(KEY|SECRET|TOKEN|PASSWORD)|ARG.*[A-Z0-9_]*(KEY|SECRET|TOKEN|PASSWORD)' "$DF"; then
  die "Edge Dockerfile bakes a secret — Zero-Trust Image violation (ADR-0004)"
fi
pass "Edge Dockerfile: multi-stage, PWA Bundle baked in, Zero-Trust (no secrets)"

# --- Caddyfile: :80 (auto-HTTPS off), /api/* -> backend, file_server --------
CF="$REPO_ROOT/infrastructure/edge/Caddyfile"
[[ -f "$CF" ]] || die "missing $CF"
grep -qE '^:80\b' "$CF" || die "Caddyfile must bind :80 (auto-HTTPS off for local dev)"
grep -qE 'handle_path[[:space:]]+/api/\*' "$CF" || die "Caddyfile must handle_path /api/* (strip prefix)"
grep -q 'http://backend:8080' "$CF" || die "Caddyfile must reverse_proxy http://backend:8080 (ADR-0006)"
grep -q 'file_server' "$CF" || die "Caddyfile must serve the PWA Bundle via file_server"
pass "Caddyfile: :80, handle_path /api/* -> http://backend:8080, file_server"

if [[ $LIVE -eq 0 ]]; then
  echo
  echo "structural tests passed (run with --live to boot the stack)"
  exit 0
fi

# --- live: build, up, curl, down ---------------------------------------------
need curl
echo "building + bringing up the stack (slow on first build)..."
docker compose up -d --build
trap 'docker compose down -v >/dev/null 2>&1 || true' EXIT

wait_http() {
  local url="$1" i
  for i in $(seq 1 90); do
    if curl -fsS -o /dev/null "$url"; then return 0; fi
    sleep 1
  done
  return 1
}

wait_http http://localhost:80/ || die "Edge never responded on :80"

html="$(curl -fsS http://localhost:80/)"
[[ "$html" == *"<html"* ]] || die "GET / did not return HTML (PWA Bundle)"
pass "GET / returns the PWA Bundle (Caddy file_server)"

if ! wait_http http://localhost:80/api/health; then
  die "/api/health never returned (backend not up behind the Edge)"
fi
health="$(curl -fsS http://localhost:80/api/health)"
[[ "$health" == *'"ok"'* && "$health" == *'"db"'* ]] \
  || die "/api/health did not reach the backend; got: $health"
pass "GET /api/health reaches the backend (reverse-proxy, /api stripped)"

ps_out="$(docker compose ps --format '{{.Service}}|{{.Ports}}')"
if printf '%s\n' "$ps_out" | awk -F'|' '$1=="backend"{print $2}' | grep -q '\->'; then
  die "backend must have no published host port; got: $(printf '%s\n' "$ps_out" | awk -F'|' '$1=="backend"{print $2}')"
fi
pass "backend has no published host port (internal-only, ADR-0006)"

docker compose exec -T backend sh -c 'test -f /data/second_brain.db' \
  || die "Brain File not present at /data/second_brain.db"
pass "Brain File present at /data/second_brain.db"

docker compose down >/dev/null
docker compose up -d >/dev/null
wait_http http://localhost:80/api/health || die "backend did not come back after down/up"
docker compose exec -T backend sh -c 'test -f /data/second_brain.db' \
  || die "Brain File did not persist across down/up"
pass "Brain File persists on sqlite_data across down/up"

docker compose down -v >/dev/null
docker compose up -d >/dev/null
wait_http http://localhost:80/api/health || true
if docker compose exec -T backend sh -c 'test -f /data/second_brain.db' 2>/dev/null; then
  die "down -v must destroy the Brain File volume"
fi
pass "down -v destroys the Brain File volume"
echo
echo "live tests passed"
