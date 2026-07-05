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
# Read-write Brain File volume: Litestream v0.5.x appends only to its own
# _litestream_* tracking tables in the source DB to record WAL position (it
# never mutates braindump/concept/edge rows), so /data must be rw — a read-only
# mount crash-loops with `attempt to write a readonly database (8)` once real R2
# credentials are present.
rw = [v for v in ls.get("volumes", []) if isinstance(v, dict) and v.get("target") == "/data"]
assert rw and rw[0].get("read_only") is not True and rw[0].get("source") == "sqlite_data", \
    f"litestream must mount sqlite_data at /data read-write; got {ls.get('volumes')}"
# Metrics loopback-only (so the host cron Health Push can reach it, nothing else can).
ports = ls.get("ports", [])
assert any(p.get("target") == 9090 and p.get("host_ip") == "127.0.0.1" for p in ports), \
    f"litestream :9090 must bind 127.0.0.1 only; got {ports}"
# No secret in the image's environment — only env_file (ADR-0004). A literal
# LITESTREAM_* key=value in `environment:` would be a baked secret.
env = ls.get("environment") or {}
bad = [k for k in env if "LITESTREAM" in k.upper() or "SECRET" in k.upper() or "ACCESS_KEY" in k.upper()]
assert not bad, f"litestream environment bakes a secret ({bad}); use env_file only (ADR-0004)"
print("ok   - compose litestream: upstream pinned image, sqlite_data rw, :9090 loopback, no baked secret")
PY
pass "Brain Replica structural shape (ADR-0002 / #32)"

if [[ $LIVE -eq 0 ]]; then
  echo
  echo "replica structural tests passed (run with --live for the MinIO round-trip)"
  exit 0
fi

# --- live: replicate -> restore round-trip with MinIO (R2 stand-in) ----------
# Verifies the core Brain Replica mechanism (#32) and the exact restore command
# from DISASTER_RECOVERY.md (#34): create a Brain-File-shaped sqlite db with a
# marker row, stream it to MinIO via litestream, destroy the local copy, restore
# from the replica, and confirm the marker survived. No backend/auth/HTTPS
# needed (POST /braindumps is session-gated and WebAuthn needs a secure context),
# so this tests the replica mechanism in isolation; the full-stack braindump
# round-trip is the manual DR exercise in DISASTER_RECOVERY.md.
need curl
echo ">> live round-trip: litestream replicate -> restore via MinIO (R2 stand-in)"
echo ">> (pulls litestream + minio + alpine; no backend build)"

PROJECT="sb-replica-test"
MINIO_BUCKET="brain-replica-test"
VOL="sb-test-data"
TMPDIR_LIVE="$(mktemp -d)"
# shellcheck disable=SC2064
trap 'docker rm -f sb-minio sb-ls >/dev/null 2>&1 || true; docker volume rm "$VOL" >/dev/null 2>&1 || true; docker network rm "$PROJECT" >/dev/null 2>&1 || true; rm -rf "$TMPDIR_LIVE"' EXIT
docker volume create "$VOL" >/dev/null 2>&1 || true
docker network create "$PROJECT" >/dev/null 2>&1 || true
docker rm -f sb-minio sb-ls >/dev/null 2>&1 || true

# MinIO as a throwaway R2 stand-in. Credentials are fixed defaults — local test
# network only, never production.
docker run -d --name sb-minio --network "$PROJECT" -p 19090:9000 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  minio/minio server /data >/dev/null
for i in $(seq 1 30); do
  curl -fsS -o /dev/null http://127.0.0.1:19090/minio/health/live && break
  sleep 1
done
curl -fsS -o /dev/null http://127.0.0.1:19090/minio/health/live || die "MinIO never became healthy"
docker run --rm --network "$PROJECT" minio/mc:latest \
  alias set local http://sb-minio:9000 minioadmin minioadmin >/dev/null 2>&1
docker run --rm --network "$PROJECT" minio/mc:latest \
  mb "local/$MINIO_BUCKET" >/dev/null 2>&1 || true

# litestream config pointed at MinIO (endpoint is NOT a s3:// URL query param, so
# a config file is required for S3-compatible targets — same shape as the
# committed infrastructure/litestream.yml, just MinIO instead of R2).
LSCONF="$TMPDIR_LIVE/litestream.yml"
cat > "$LSCONF" <<EOF
dbs:
  - path: /data/second_brain.db
    replica:
      type: s3
      bucket: $MINIO_BUCKET
      endpoint: http://sb-minio:9000
      path: second_brain.db
      region: us-east-1
      sync-interval: 1s
EOF

# 1. Create a Brain-File-shaped sqlite db with a marker row on the volume.
MARK="replica-test-$(date +%s)"
docker run --rm -v "$VOL":/data alpine:3 sh -c \
  "apk add --no-cache sqlite >/dev/null 2>&1 && sqlite3 /data/second_brain.db \"CREATE TABLE IF NOT EXISTS braindumps(id INTEGER PRIMARY KEY, verbatim TEXT); INSERT INTO braindumps(verbatim) VALUES('$MARK');\"" \
  || die "could not create the marker sqlite db"

# 2. Stream the WAL to MinIO (litestream snapshots the existing db on start).
docker run -d --name sb-ls --network "$PROJECT" \
  -v "$VOL":/data -v "$LSCONF":/etc/litestream.yml:ro \
  -e LITESTREAM_ACCESS_KEY_ID=minioadmin -e LITESTREAM_SECRET_ACCESS_KEY=minioadmin \
  litestream/litestream:0.5.13 replicate >/dev/null \
  || die "could not start litestream replicate"
echo ">> waiting for Litestream to sync the WAL to MinIO (RPO ~1s + margin)..."
sleep 6
docker stop sb-ls >/dev/null

# Confirm replica objects landed in the bucket.
docker run --rm --network "$PROJECT" minio/mc:latest \
  find "local/$MINIO_BUCKET" --recursive >/dev/null 2>&1 \
  || die "no replica objects in MinIO bucket — replication did not land"

# 3. Destroy the local Brain File, then restore it from the replica (the exact
#    command shape from DISASTER_RECOVERY.md step 3, #34).
echo ">> destroying local Brain File and restoring from the replica..."
docker run --rm -v "$VOL":/data alpine:3 sh -c \
  'rm -f /data/second_brain.db /data/second_brain.db-wal /data/second_brain.db-shm' || true
docker run --rm --network "$PROJECT" \
  -v "$VOL":/data -v "$LSCONF":/etc/litestream.yml:ro \
  -e LITESTREAM_ACCESS_KEY_ID=minioadmin -e LITESTREAM_SECRET_ACCESS_KEY=minioadmin \
  litestream/litestream:0.5.13 restore -config /etc/litestream.yml /data/second_brain.db \
  || die "litestream restore from MinIO failed"

# 4. The restored brain must contain the marker row.
OUT="$(docker run --rm -v "$VOL":/data alpine:3 sh -c \
  'apk add --no-cache sqlite >/dev/null 2>&1 && sqlite3 /data/second_brain.db "SELECT verbatim FROM braindumps"')"
if [[ "$OUT" == *"$MARK"* ]]; then
  pass "replicate -> restore round-trip: marker row survived (R2 stand-in via MinIO)"
else
  die "restored Brain File is missing the marker row — replica round-trip failed (got: $OUT)"
fi

echo
echo "replica live round-trip passed"

