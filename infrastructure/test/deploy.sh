#!/usr/bin/env bash
# Structural tests for the deploy pipeline - the command-restricted SSH deploy
# entrypoint (deploy.sh) + the bootstrap ownership split (ADR-0003 / ADR-0007 /
# ADR-0010).
#
#   bash infrastructure/test/deploy.sh            # structural (fast, no network)
#
# Asserts:
#   - deploy.sh keeps the ADR-0007 deploy.env whitelist (REGISTRY/EDGE_TAG/
#     BACKEND_TAG only) and the SHA extraction that drives config sync.
#   - deploy.sh syncs config by FETCHING from raw.githubusercontent.com (public
#     repo) at the deployed SHA - never by accepting piped config content - so a
#     leaked SSH key can replay but not craft config (ADR-0003 invariant).
#   - deploy.sh is Zero-Trust: references NO runtime secret (ADR-0004). It pulls
#     from a public repo with no token, so no secret is needed on the VPS or GHA.
#   - bootstrap.sh installs the 3 sync-eligible config files as deploy-owned
#     (so deploy.sh, running as deploy, can overwrite them) but keeps deploy.sh
#     itself root-owned and the install dir root-owned - the gate the deploy key
#     cannot replace (ADR-0010).
# Complements deploy.sh --self-test (behavioral, mocked) which is invoked
# separately in CI. Uses grep only - no third-party deps, runs in CI.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

pass() { printf 'ok   - %s\n' "$*"; }
die()  { printf 'FAIL - %s\n' "$*" >&2; exit 1; }

DEPLOY="$REPO_ROOT/infrastructure/deploy.sh"
BOOT="$REPO_ROOT/infrastructure/bootstrap.sh"
[[ -f "$DEPLOY" ]] || die "missing $DEPLOY"
[[ -f "$BOOT"  ]] || die "missing $BOOT"

# --- deploy.sh: ADR-0007 whitelist still present ------------------------------
grep -qE "validate_deploy_env|REGISTRY=.*EDGE_TAG=.*BACKEND_TAG" "$DEPLOY" \
  || die "deploy.sh must define the deploy.env whitelist validation (ADR-0007)"
grep -qF 'for k in REGISTRY EDGE_TAG BACKEND_TAG' "$DEPLOY" \
  || die "deploy.sh must require all three Deploy State keys (ADR-0007)"
# A non-whitelisted key must be rejected (the regex guard).
grep -qF "not whitelisted Deploy State" "$DEPLOY" \
  || die "deploy.sh must log+reject non-whitelisted deploy.env lines"
pass "deploy.sh keeps the ADR-0007 deploy.env whitelist (3 keys, reject extras)"

# --- deploy.sh: config sync fetches from the public repo at the SHA -----------
grep -qF 'raw.githubusercontent.com' "$DEPLOY" \
  || die "deploy.sh must sync config from raw.githubusercontent.com (ADR-0010)"
grep -qF 'BACKEND_TAG' "$DEPLOY" && grep -qE 'sha-\*.*git_sha=' "$DEPLOY" \
  || die "deploy.sh must extract the git SHA from BACKEND_TAG (sha-<sha>)"
# The three sync-eligible files.
grep -qF 'docker-compose.yml:644' "$DEPLOY" \
  || die "deploy.sh sync set must include docker-compose.yml:644"
grep -qF 'infrastructure/litestream.yml:644' "$DEPLOY" \
  || die "deploy.sh sync set must include infrastructure/litestream.yml:644"
grep -qF 'infrastructure/health-push.sh:755' "$DEPLOY" \
  || die "deploy.sh sync set must include infrastructure/health-push.sh:755"
# Fetch-all-before-install (atomicity): a 404 must abort before any install.
grep -qF 'Fetch ALL first' "$DEPLOY" \
  || die "deploy.sh must fetch all config files before installing any (atomic sync)"
pass "deploy.sh syncs config by FETCHING from the public repo at the deployed SHA (ADR-0010)"

# --- deploy.sh: Zero-Trust - references NO runtime secret (ADR-0004) ----------
# deploy.sh pulls from a PUBLIC repo with no auth; it must not read, expand, or
# reference any [SECRET] key from .env.example. Naming one in a comment would be
# a leak vector if the comment drifted into a real reference, so assert absence.
for secret in GEMINI_API_KEY LITESTREAM_ACCESS_KEY_ID LITESTREAM_SECRET_ACCESS_KEY NTFY_WEBHOOK_URL; do
  if grep -qF "$secret" "$DEPLOY"; then
    die "deploy.sh references secret $secret - Zero-Trust violation (ADR-0004): config sync must need no secret (public repo)"
  fi
done
pass "deploy.sh is Zero-Trust: no runtime-secret reference (public-repo fetch needs no token, ADR-0004)"

# --- deploy.sh: the gate must NOT be in its own sync set ----------------------
# deploy.sh cannot sync itself (mid-execution chicken-and-egg); it must stay
# root-owned and manual. Assert 'deploy.sh' is not a SYNC_FILES entry.
if grep -Eq 'SYNC_FILES=.*"deploy\.sh' "$DEPLOY"; then
  die "deploy.sh must NOT sync itself (chicken-and-egg; it is the root-owned gate, manual update only)"
fi
pass "deploy.sh does not self-sync (root-owned gate, updated manually like .env)"

# --- bootstrap.sh: 3 config files deploy-owned so deploy.sh can overwrite -----
# Overwriting an existing file you own needs write perm on the FILE only, so a
# deploy-owned file in a root-owned dir is overwritable by deploy but not
# deletable. bootstrap must install the sync-eligible files as -o $DEPLOY_USER.
for f in docker-compose.yml infrastructure/litestream.yml infrastructure/health-push.sh; do
  if ! grep -Eq "install .*-o \"\\\$DEPLOY_USER\" .*\"\\\$REPO_ROOT/$f\"" "$BOOT"; then
    die "bootstrap.sh must install $f as -o \$DEPLOY_USER (deploy-owned) so deploy.sh can overwrite it (ADR-0010)"
  fi
done
pass "bootstrap.sh installs the 3 sync-eligible config files as deploy-owned (ADR-0010)"

# --- bootstrap.sh: deploy.sh + install dir stay root-owned (the gate) ---------
# If deploy.sh were deploy-owned (or the install dir deploy-writable), the deploy
# key could replace the validation gate - neutering ADR-0007. Both must be root.
if ! grep -Eq 'install -m 755 -o root -g root "\$REPO_ROOT/infrastructure/deploy\.sh"' "$BOOT"; then
  die "bootstrap.sh must install deploy.sh as -o root (the gate must NOT be deploy-writable, ADR-0010)"
fi
if ! grep -Eq 'install -d -o root[[:space:]]+-g root[[:space:]]+-m 755 "\$INSTALL_DIR"' "$BOOT"; then
  die "bootstrap.sh must keep \$INSTALL_DIR root-owned (else deploy could create files / replace deploy.sh)"
fi
pass "bootstrap.sh keeps deploy.sh + \$INSTALL_DIR root-owned (the gate the deploy key cannot replace)"

echo "deploy structural tests passed"
