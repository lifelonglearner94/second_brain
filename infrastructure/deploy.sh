#!/usr/bin/env bash
# Server-side deploy entrypoint — the forced command in the deploy user's
# authorized_keys (ADR-0003). The command restriction *is* the security model:
# the GHA deploy key can only run this script — no shell, no pty, no port
# forwarding, no agent forwarding. If the key leaks, an attacker can only
# trigger a pull + restart of the stack with a tag that must already exist in
# GHCR (which they cannot push without repo write access).
#
# GitHub Actions pipes deploy.env over SSH stdin; this script:
#   1. reads it,
#   2. validates it against a strict Deploy State whitelist (ADR-0007) so a
#      leaked key cannot repurpose this script to write arbitrary content,
#   3. writes /opt/second-brain/deploy.env,
#   4. syncs the non-secret infra config files (docker-compose.yml,
#      litestream.yml, health-push.sh) from the PUBLIC repo at the deployed
#      commit SHA (ADR-0010), so the running config always matches the running
#      images — closes the gap that left a stale read-only litestream mount on
#      the VPS long after the repo fix shipped (R2 stayed empty),
#   5. runs `docker compose pull && up -d` with those tags (ADR-0003).
#
# Config sync fetches (not accepts piped content) from raw.githubusercontent.com
# at the SHA in BACKEND_TAG. The repo is PUBLIC, so no auth and no new secret is
# needed on the VPS or in GHA. This preserves ADR-0003's invariant: an attacker
# with only the leaked SSH key can REPLAY past configs (any real commit SHA) but
# cannot CRAFT novel ones — the config must exist in the repo. (Piping a crafted
# tar over stdin was rejected: it would let a leaked key run arbitrary compose,
# bind-mount .env, and exfiltrate secrets — breaking the command-restriction
# model. See ADR-0010 for the full rejected-alternatives list.)
#
# Secrets (.env) are never touched here — GHA is blind to them (ADR-0004).
# deploy.sh itself is NOT in the sync set (mid-execution chicken-and-egg); it
# stays root-owned and is updated manually like .env (one-time scp + bootstrap).
#
# Self-test:  bash infrastructure/deploy.sh --self-test
#   (mocks the fetch + docker steps — no network, no VPS, no docker needed)
set -euo pipefail

# --- config (env-overridable for --self-test; prod defaults are fixed) ---------
# Resolved by config_init() at the start of main()/self_test() so test overrides
# (SB_COMPOSE_DIR, SB_GITHUB_REPO) take effect even when set after the script
# loads. Defaults are the real production paths; an attacker reaching this script
# via the forced command gets no shell and cannot set env (sshd AcceptEnv defaults
# to none), so the defaults are effectively immutable in production.
COMPOSE_DIR=""
COMPOSE_FILE=""
DEPLOY_ENV=""
LOG_FILE=""
GITHUB_REPO="${SB_GITHUB_REPO:-lifelonglearner94/second_brain}"
RAW_BASE=""
config_init() {
  COMPOSE_DIR="${SB_COMPOSE_DIR:-/opt/second-brain}"
  COMPOSE_FILE="$COMPOSE_DIR/docker-compose.yml"
  DEPLOY_ENV="$COMPOSE_DIR/deploy.env"
  LOG_FILE="$COMPOSE_DIR/deploy.log"
  GITHUB_REPO="${SB_GITHUB_REPO:-lifelonglearner94/second_brain}"
  RAW_BASE="https://raw.githubusercontent.com/${GITHUB_REPO}"
}
config_init   # resolve defaults once (re-resolved by main/self_test as needed)

log() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$LOG_FILE" >&2; }

# Non-secret infra config files synced from the public repo at the deployed SHA
# on every deploy (ADR-0010). These are the files bootstrap.sh installs once at
# setup; without this sync they bitrot on the VPS. Format: "repo-path:mode".
# NOT synced: deploy.sh itself (root-owned gate, manual), infrastructure/.env
# (secrets, ADR-0004), infrastructure/health-push.cron (system /etc/cron.d path,
# rare, manual bootstrap re-run).
SYNC_FILES=(
  "docker-compose.yml:644"
  "infrastructure/litestream.yml:644"
  "infrastructure/health-push.sh:755"
)

# --- seam: fetch one config file at a repo-relative path + SHA -----------------
# Prod: curl raw.githubusercontent.com (public repo, no auth, no secret).
# Self-test: SB_MOCK_CONFIG_DIR set -> cp from there (no network).
fetch_config() {  # $1=repo_path $2=sha $3=out_file
  local repo_path="$1" sha="$2" out="$3"
  if [[ -n "${SB_MOCK_CONFIG_DIR:-}" ]]; then
    cp "$SB_MOCK_CONFIG_DIR/$repo_path" "$out"
    return  # propagate cp's status (mocks a 404 when the file is absent)
  fi
  command -v curl >/dev/null 2>&1 || { log "curl required for config sync (bootstrap installs it)"; return 1; }
  curl -fsSL -o "$out" "${RAW_BASE}/${sha}/${repo_path}"
}

# --- validate deploy.env: strict Deploy State whitelist (ADR-0007) -------------
# Pure: $1 = file to check. Returns 0 if valid, 1 (with log) if invalid.
# ONLY REGISTRY/EDGE_TAG/BACKEND_TAG are permitted; a leaked key cannot write
# arbitrary content. All three required (a garbled stdin must NOT blank
# deploy.env and pull a stale :latest tag).
validate_deploy_env() {  # $1 = file
  local f="$1"
  if grep -nEv '^(REGISTRY=[A-Za-z0-9._/:-]*|EDGE_TAG=[A-Za-z0-9._:-]*|BACKEND_TAG=[A-Za-z0-9._:-]*|[[:space:]]*#.*|[[:space:]]*)$' "$f" > /tmp/deploy-reject.$$ 2>/dev/null; then
    log "rejected deploy.env lines (not whitelisted Deploy State):"
    sed 's/^/  /' /tmp/deploy-reject.$$ | tee -a "$LOG_FILE" >&2
    rm -f /tmp/deploy-reject.$$
    return 1
  fi
  rm -f /tmp/deploy-reject.$$
  for k in REGISTRY EDGE_TAG BACKEND_TAG; do
    if ! grep -qE "^${k}=[A-Za-z0-9._/:-]+$" "$f"; then
      log "rejected: deploy.env missing required key ${k} (refusing to write)"
      return 1
    fi
  done
}

# --- pull pinned images + bring up the stack (seam: mocked in --self-test) ----
deploy_up() {
  if [[ -n "${SB_MOCK_DEPLOY:-}" ]]; then
    log "(mock) docker compose pull && up -d"
    return 0
  fi
  cd "$COMPOSE_DIR"
  log "docker compose pull"
  docker compose --env-file "$DEPLOY_ENV" -f "$COMPOSE_FILE" pull
  log "docker compose up -d"
  docker compose --env-file "$DEPLOY_ENV" -f "$COMPOSE_FILE" up -d
}

# --- main deploy (stdin = deploy.env piped by GHA) ----------------------------
# tmp/TMP_SYNC are intentionally NOT `local` — the EXIT trap (which fires after
# main returns, in global scope) must see them to clean up on an early exit.
tmp=""
TMP_SYNC=""
main() {
  config_init   # re-resolve paths so SB_* test overrides take effect
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"; [ -n "$TMP_SYNC" ] && rm -rf "$TMP_SYNC"' EXIT
  cat > "$tmp"

  # 1-3. Validate + write deploy.env (deploy-owned; cp preserves ownership).
  validate_deploy_env "$tmp" || exit 1
  cp "$tmp" "$DEPLOY_ENV"
  chmod 600 "$DEPLOY_ENV"
  log "wrote deploy.env: $(grep -E '^(EDGE|BACKEND)_TAG=' "$DEPLOY_ENV" | paste -sd ' ' -)"

  # 4. Sync infra config from the public repo at the deployed commit SHA.
  #    SHA is extracted from BACKEND_TAG (sha-<gitsha> -> <gitsha>).
  local backend_tag git_sha=""
  backend_tag="$(grep -E '^BACKEND_TAG=' "$DEPLOY_ENV" | head -1 | cut -d= -f2-)"
  case "$backend_tag" in
    sha-*) git_sha="${backend_tag#sha-}" ;;
    *) log "skip config sync: BACKEND_TAG not sha-pinned ($backend_tag)" ;;
  esac

  if [[ -n "$git_sha" ]]; then
    TMP_SYNC="$(mktemp -d)"
    local entry repo_path mode out dest
    # Fetch ALL first — a single 404 aborts before any file is touched (atomic).
    for entry in "${SYNC_FILES[@]}"; do
      repo_path="${entry%:*}"
      out="$TMP_SYNC/$(echo "$repo_path" | tr / _)"
      if ! fetch_config "$repo_path" "$git_sha" "$out" 2>/dev/null; then
        log "config sync FAILED: could not fetch ${repo_path} @ ${git_sha} (repo must be public + SHA must exist)"
        exit 1
      fi
    done
    # All fetched OK — install. cp preserves the deploy ownership set by
    # bootstrap (the files pre-exist deploy-owned); chmod re-asserts the mode.
    for entry in "${SYNC_FILES[@]}"; do
      repo_path="${entry%:*}"; mode="${entry#*:}"
      out="$TMP_SYNC/$(echo "$repo_path" | tr / _)"
      dest="$COMPOSE_DIR/$repo_path"
      mkdir -p "$(dirname "$dest")"
      cp "$out" "$dest" && chmod "$mode" "$dest"
      log "synced ${repo_path} @ ${git_sha}"
    done
  fi

  # 5. Pull the pinned images and bring up the stack.
  deploy_up
  log "deploy complete"
  rm -f "$tmp"; [ -n "$TMP_SYNC" ] && rm -rf "$TMP_SYNC"
}

# --- self-test (RED/GREEN without network, VPS, or docker) --------------------
self_test() {
  local tmp mock
  tmp="$(mktemp -d)"
  mock="$tmp/repo"
  mkdir -p "$mock/infrastructure"
  printf 'compose-stub\n' > "$mock/docker-compose.yml"
  printf 'litestream-stub\n' > "$mock/infrastructure/litestream.yml"
  printf '#!/bin/sh\necho hp\n' > "$mock/infrastructure/health-push.sh"

  export SB_COMPOSE_DIR="$tmp/opt" SB_MOCK_CONFIG_DIR="$mock" SB_MOCK_DEPLOY=1
  mkdir -p "$SB_COMPOSE_DIR/infrastructure"
  : > "$SB_COMPOSE_DIR/deploy.log"
  config_init   # re-resolve paths NOW that SB_* overrides are in place

  pass() { printf 'ok   - %s\n' "$*"; }
  die()  { printf 'FAIL - %s\n' "$*" >&2; exit 1; }

  # 1. valid deploy.env accepted
  printf 'REGISTRY=ghcr.io/x/\nEDGE_TAG=sha-abc\nBACKEND_TAG=sha-abc123\n' > "$tmp/good.env"
  validate_deploy_env "$tmp/good.env" && pass "valid deploy.env accepted" || die "valid deploy.env rejected"

  # 2. extra (non-whitelisted) key rejected
  printf 'REGISTRY=ghcr.io/x/\nEDGE_TAG=sha-abc\nBACKEND_TAG=sha-abc\nEVIL=1\n' > "$tmp/evil.env"
  if validate_deploy_env "$tmp/evil.env" 2>/dev/null; then die "extra key accepted (whitelist broken)"; fi
  pass "non-whitelisted key rejected"

  # 3. missing required key rejected
  printf 'REGISTRY=ghcr.io/x/\nEDGE_TAG=sha-abc\n' > "$tmp/missing.env"
  if validate_deploy_env "$tmp/missing.env" 2>/dev/null; then die "missing BACKEND_TAG accepted"; fi
  pass "missing required key rejected"

  # 4. SHA extraction from BACKEND_TAG
  local bt gs=""
  bt="sha-abc123def"; case "$bt" in sha-*) gs="${bt#sha-}" ;; *) gs="" ;; esac
  [[ "$gs" == "abc123def" ]] || die "SHA extraction wrong: '$gs'"
  pass "git SHA extracted from BACKEND_TAG (sha-<sha> -> <sha>)"

  # 5. full main() with mock config -> all 3 files synced with correct content
  printf 'REGISTRY=ghcr.io/x/\nEDGE_TAG=sha-deadbeef\nBACKEND_TAG=sha-deadbeef\n' | main >/dev/null 2>&1 \
    || die "main() failed with valid deploy.env + mock config"
  [[ -f "$SB_COMPOSE_DIR/docker-compose.yml" ]] || die "docker-compose.yml not synced"
  [[ -f "$SB_COMPOSE_DIR/infrastructure/litestream.yml" ]] || die "litestream.yml not synced"
  [[ -f "$SB_COMPOSE_DIR/infrastructure/health-push.sh" ]] || die "health-push.sh not synced"
  [[ "$(cat "$SB_COMPOSE_DIR/docker-compose.yml")" == "compose-stub" ]] || die "synced content mismatch"
  pass "main() syncs all 3 config files from the repo at the deployed SHA"

  # 6. a missing config file aborts BEFORE any install (atomicity)
  rm "$mock/infrastructure/health-push.sh"
  rm -f "$SB_COMPOSE_DIR/docker-compose.yml" "$SB_COMPOSE_DIR/infrastructure/litestream.yml"
  if printf 'REGISTRY=ghcr.io/x/\nEDGE_TAG=sha-deadbeef\nBACKEND_TAG=sha-deadbeef\n' | main >/dev/null 2>&1; then
    die "main() succeeded with a missing config file (should have aborted)"
  fi
  [[ ! -f "$SB_COMPOSE_DIR/docker-compose.yml" ]] || die "partial install after fetch failure (atomicity broken)"
  pass "fetch failure aborts before any file is installed (atomic sync)"

  rm -rf "$tmp"
  echo "deploy self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
else
  main
fi
