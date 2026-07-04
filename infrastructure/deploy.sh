#!/usr/bin/env bash
# Server-side deploy entrypoint — the forced command in the deploy user's
# authorized_keys (ADR-0003). The command restriction *is* the security model:
# the GHA deploy key can only run this script — no shell, no pty, no port
# forwarding, no agent forwarding. If the key leaks, an attacker can only
# trigger a pull + restart of the existing stack with a tag that must already
# exist in GHCR (which they cannot push without repo write access).
#
# GitHub Actions pipes the new deploy.env over SSH stdin; this script:
#   1. reads it,
#   2. validates it against a strict Deploy State whitelist (ADR-0007) so a
#      leaked key cannot repurpose this script to write arbitrary files,
#   3. writes /opt/second-brain/deploy.env,
#   4. runs `docker compose pull && up -d` with those tags (ADR-0003).
#
# Secrets (.env) are never touched here — GHA is blind to them (ADR-0004).
set -euo pipefail

COMPOSE_DIR="/opt/second-brain"
COMPOSE_FILE="$COMPOSE_DIR/docker-compose.yml"
DEPLOY_ENV="$COMPOSE_DIR/deploy.env"
LOG_FILE="$COMPOSE_DIR/deploy.log"

log() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$LOG_FILE" >&2; }

# 1. Consume the new deploy.env from stdin (GHA pipes it).
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
cat > "$tmp"

# 2. Strict validation: ONLY Deploy State keys are permitted (ADR-0007).
#    REGISTRY = ghcr path prefix, *_TAG = SHA-pinned image tags.
#    Reject any other line so a leaked key cannot write arbitrary content.
#    Also require all three keys present — a garbled/empty stdin must NOT
#    blank deploy.env and then pull a stale :latest tag.
if grep -nEv '^(REGISTRY=[A-Za-z0-9._/:-]*|EDGE_TAG=[A-Za-z0-9._:-]*|BACKEND_TAG=[A-Za-z0-9._:-]*|[[:space:]]*#.*|[[:space:]]*)$' "$tmp" > /tmp/deploy-reject.$$ 2>/dev/null; then
  log "rejected deploy.env lines (not whitelisted Deploy State):"
  sed 's/^/  /' /tmp/deploy-reject.$$ | tee -a "$LOG_FILE" >&2
  rm -f /tmp/deploy-reject.$$
  exit 1
fi
rm -f /tmp/deploy-reject.$$
for k in REGISTRY EDGE_TAG BACKEND_TAG; do
  if ! grep -qE "^${k}=[A-Za-z0-9._/:-]+$" "$tmp"; then
    log "rejected: deploy.env missing required key ${k} (refusing to write)"
    exit 1
  fi
done

# 3. Write deploy.env (owned by the deploy user; cp preserves ownership).
cp "$tmp" "$DEPLOY_ENV"
chmod 600 "$DEPLOY_ENV"
log "wrote deploy.env: $(grep -E '^(EDGE|BACKEND)_TAG=' "$DEPLOY_ENV" | paste -sd ' ' -)"

# 4. Pull the pinned images and bring up the stack.
cd "$COMPOSE_DIR"
log "docker compose pull"
docker compose --env-file "$DEPLOY_ENV" -f "$COMPOSE_FILE" pull
log "docker compose up -d"
docker compose --env-file "$DEPLOY_ENV" -f "$COMPOSE_FILE" up -d
log "deploy complete"
