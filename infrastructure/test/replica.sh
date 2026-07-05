#!/usr/bin/env bash
# Structural (and optional live) tests for the Brain Replica — the Litestream
# sidecar that tails the Brain File WAL to Cloudflare R2 (ADR-0002, issue #32).
#
#   bash infrastructure/test/replica.sh           # structural (fast, no build)
#   bash infrastructure/test/replica.sh --live    # full replicate->restore round-trip
#
# Structural tests assert the litestream Compose service + litestream.yml shape
# and a config-file Zero-Trust rule: litestream.yml must reference NO secret
# (R2 creds are auto-read from env, ADR-0004) — complementing zero-trust.sh,
# which scans Dockerfiles only. Live tests boot MinIO (a local R2 stand-in),
# submit a braindump, wait for sync, stop the backend, `litestream restore` into
# a fresh volume, and confirm the braindump survived — the #32 DR dry run.
# Uses Python stdlib (json/yaml via compose config) + grep — no third-party deps.
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

# --- litestream.yml: shape + Zero-Trust (no secret reference) ----------------
# Asserted with grep (not PyYAML) to keep CI dependency-free, matching the
# topology.sh convention. .env.example's [SECRET] legend is the secret key list.
CFG="$REPO_ROOT/infrastructure/litestream.yml"
[[ -f "$CFG" ]] || die "missing $CFG (ADR-0002 / #32)"

# Shape: metrics addr, one db at the Brain File path, s3 -> R2, 1s sync.
grep -qE '^addr:[[:space:]]*":9090"' "$CFG" \
  || die "litestream.yml must set addr \":9090\" (metrics, ADR-0005)"
grep -qE '^[[:space:]]*-?[[:space:]]*path:[[:space:]]+/data/second_brain\.db' "$CFG" \
  || die "litestream.yml must replicate /data/second_brain.db (the Brain File)"
grep -qE '^[[:space:]]*type:[[:space:]]*s3' "$CFG" \
  || die "litestream.yml replica type must be s3 (R2 is S3-compatible)"
grep -qF 'bucket: ${LITESTREAM_BUCKET}' "$CFG" \
  || die "litestream.yml bucket must \${LITESTREAM_BUCKET}-expand from .env"
grep -qF 'endpoint: ${LITESTREAM_ENDPOINT}' "$CFG" \
  || die "litestream.yml endpoint must \${LITESTREAM_ENDPOINT}-expand from .env"
grep -qE '^[[:space:]]*sync-interval:[[:space:]]*1s' "$CFG" \
  || die "litestream.yml sync-interval must be 1s (RPO ~1s, ADR-0002)"
pass "litestream.yml: /data/second_brain.db -> s3 R2, addr :9090, sync 1s"

# Zero-Trust (ADR-0004): the committed config must NOT reference the secret keys
# in an active field or ${VAR} expansion — Litestream auto-reads
# LITESTREAM_ACCESS_KEY_ID / LITESTREAM_SECRET_ACCESS_KEY from env. We match a
# `access-key-id:` / `secret-access-key:` field (active OR commented-out literal
# — a commented secret is still a leak) or a ${SECRET_VAR} expansion. Naming the
# env var in a doc comment is allowed (no ${} and no field colon).
if grep -iEn 'access-key-id:|secret-access-key:|\$\{(LITESTREAM_ACCESS_KEY_ID|LITESTREAM_SECRET_ACCESS_KEY|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY)\}' "$CFG"; then
  die "litestream.yml references a secret key — Zero-Trust violation (ADR-0004): creds must be env-auto-read, never in the committed config"
fi
pass "litestream.yml is Zero-Trust: no secret reference (creds env-auto-read, ADR-0004)"

# --- Compose service shape (resolved JSON) -----------------------------------
docker compose config >/dev/null
python3 - "$REPO_ROOT" <<'PY'
import json, subprocess, sys
repo = sys.argv[1]
cfg = json.loads(subprocess.check_output(
    ["docker", "compose", "config", "--format", "json"], cwd=repo, text=True))
svc = cfg["services"]
assert "litestream" in svc, "compose must define a litestream service (ADR-0002 / #32)"
ls = svc["litestream"]
assert str(ls.get("image","")).startswith("litestream/litestream:"), \
    f"litestream must use the upstream litestream image (pinned tag); got {ls.get('image')}"
assert ls.get("command") == ["replicate"], f"litestream command must be [replicate]; got {ls.get('command')}"
# Read-only Brain File volume for WAL tailing.
ro = [v for v in ls.get("volumes", []) if isinstance(v, dict) and v.get("target") == "/data"]
assert ro and ro[0].get("read_only") is True and ro[0].get("source") == "sqlite_data", \
    f"litestream must mount sqlite_data at /data read-only; got {ls.get('volumes')}"
# Metrics loopback-only (so the host cron Health Push can reach it, nothing else can).
ports = ls.get("ports", [])
assert any(p.get("target") == 9090 and p.get("host_ip") == "127.0.0.1" for p in ports), \
    f"litestream :9090 must bind 127.0.0.1 only; got {ports}"
# No secret in the image's environment — only env_file (ADR-0004). A literal
# LITESTREAM_* key=value in `environment:` would be a baked secret.
env = ls.get("environment") or {}
bad = [k for k in env if "LITESTREAM" in k.upper() or "SECRET" in k.upper() or "ACCESS_KEY" in k.upper()]
assert not bad, f"litestream environment bakes a secret ({bad}); use env_file only (ADR-0004)"
print("ok   - compose litestream: upstream pinned image, sqlite_data ro, :9090 loopback, no baked secret")
PY
pass "Brain Replica structural shape (ADR-0002 / #32)"

if [[ $LIVE -eq 0 ]]; then
  echo
  echo "replica structural tests passed (run with --live for the MinIO round-trip)"
  exit 0
fi

# --- live: replicate -> restore round-trip with MinIO (the #32 DR dry run) ----
need curl
echo ">> live round-trip: MinIO (R2 stand-in) + stack + braindump + restore"
echo ">> (slow: builds the backend, pulls litestream + minio)"

# MinIO as a local, throwaway R2 stand-in on an isolated network. Credentials are
# fixed defaults — this is a local test network, never production.
MINIO_NET="sb-replica-test"
MINIO_BUCKET="brain-replica-test"
docker network create "$MINIO_NET" >/dev/null 2>&1 || true
docker rm -f sb-minio >/dev/null 2>&1 || true
docker run -d --name sb-minio --network "$MINIO_NET" \
  -p 19090:9000 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  -e MINIO_DOMAIN=minio \
  minio/minio server /data >/dev/null
trap 'docker rm -f sb-minio >/dev/null 2>&1 || true; docker network rm "$MINIO_NET" >/dev/null 2>&1 || true' EXIT

# Wait for MinIO, create the bucket.
for i in $(seq 1 30); do
  if curl -fsS -o /dev/null http://127.0.0.1:19090/minio/health/live; then break; fi
  sleep 1
done
curl -fsS -o /dev/null http://127.0.0.1:19090/minio/health/live || die "MinIO never became healthy"
docker run --rm --network "$MINIO_NET" minio/mc:latest \
  alias set local http://sb-minio:9000 minioadmin minioadmin >/dev/null 2>&1
docker run --rm --network "$MINIO_NET" minio/mc:latest \
  mb "local/$MINIO_BUCKET" >/dev/null 2>&1 || true

# Point litestream at MinIO via a throwaway env, then bring the stack up.
TEST_ENV="$(mktemp)"
trap 'rm -f "$TEST_ENV"; docker rm -f sb-minio >/dev/null 2>&1 || true; docker network rm "$MINIO_NET" >/dev/null 2>&1 || true; docker compose down -v >/dev/null 2>&1 || true' EXIT
cat > "$TEST_ENV" <<EOF
GEMINI_API_KEY=
LITESTREAM_ACCESS_KEY_ID=minioadmin
LITESTREAM_SECRET_ACCESS_KEY=minioadmin
LITESTREAM_ENDPOINT=http://sb-minio:9000
LITESTREAM_BUCKET=$MINIO_BUCKET
EOF
# Override env_file path by copying the test env into place (restored on EXIT).
cp "$TEST_ENV" infrastructure/.env.live-test
trap 'rm -f "$TEST_ENV" infrastructure/.env.live-test; docker rm -f sb-minio >/dev/null 2>&1 || true; docker network rm "$MINIO_NET" >/dev/null 2>&1 || true; docker compose down -v >/dev/null 2>&1 || true' EXIT

echo ">> building + bringing up the stack against MinIO (slow on first build)..."
LITESTREAM_ENDPOINT=http://sb-minio:9000 LITESTREAM_BUCKET=$MINIO_BUCKET \
  docker compose up -d --build
wait_http() { local url="$1" i; for i in $(seq 1 120); do curl -fsS -o /dev/null "$url" && return 0; sleep 1; done; return 1; }
wait_http http://localhost:80/api/health || die "backend never came up behind the Edge"

# Submit a braindump (no Gemini key -> backend fake-LLM fallback, still ingests).
MARK="replica-test-$(date +%s)"
curl -fsS -X POST http://localhost:80/api/braindumps \
  -H 'Content-Type: application/json' \
  -d "{\"text\":\"replica round-trip marker $MARK\"}" \
  || die "could not submit braindump for the round-trip"

# Let Litestream sync (RPO ~1s; give it margin + a checkpoint).
echo ">> waiting for Litestream to sync the WAL to MinIO..."
sleep 5
docker compose exec -T litestream litestream sync /data/second_brain.db >/dev/null 2>&1 || true
# Confirm frames landed in the bucket.
docker run --rm --network "$MINIO_NET" minio/mc:latest \
  find "local/$MINIO_BUCKET" --recursive >/dev/null 2>&1 \
  || die "no replica objects in MinIO bucket — replication did not land"

# Stop the backend, destroy the local volume, restore from the replica, restart.
echo ">> destroying local Brain File and restoring from the replica..."
docker compose stop backend >/dev/null
docker volume rm second_brain_sqlite_data >/dev/null 2>&1 || true
docker compose run --rm --no-deps -v second_brain_sqlite_data:/data \
  litestream litestream restore -o /data/second_brain.db \
  "s3://${MINIO_BUCKET}/second_brain.db" >/dev/null \
  || die "litestream restore from MinIO failed"

docker compose up -d backend >/dev/null
wait_http http://localhost:80/api/health || die "backend did not come back after restore"
# The restored brain must contain the marker braindump.
if curl -fsS http://localhost:80/api/braindumps | grep -q "$MARK"; then
  pass "restored Brain File contains the marker braindump (replicate->restore round-trip ok)"
else
  die "restored Brain File is missing the marker braindump — replica restore failed"
fi

docker compose down -v >/dev/null
echo
echo "replica live round-trip passed"
