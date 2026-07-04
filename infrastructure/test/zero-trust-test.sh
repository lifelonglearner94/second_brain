#!/usr/bin/env bash
# Self-test for the Zero-Trust Image guard (issue #30, TDD red/green).
#
# Two seams, exercised against the guard's public interface (exit code):
#   RED   — a throwaway Dockerfile with planted ENV/ARG/COPY secrets MUST make
#            the guard exit non-zero.
#   GREEN — the real repo Dockerfiles (backend + Edge) MUST make the guard exit
#            zero.
# The throwaway file lives outside the repo (mktemp) and is removed on exit, so
# the repo scan in the GREEN step never sees it. GHA-blind: this test never
# reads infrastructure/.env.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$REPO_ROOT/infrastructure/test/zero-trust.sh"

[[ -f "$GUARD" ]] || { echo "FAIL - guard not found at $GUARD" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# --- RED: planted secrets must FAIL -------------------------------------------
cat > "$TMP/Dockerfile" <<'EOF'
FROM alpine:3
# Planted secret literals — every directive below must be flagged by the guard.
ARG GEMINI_API_KEY=AIzaSyFAKESECRET1234567890abcdef
ENV AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
ENV benign_named_var=c9f8e7d6a5b4a3f2e1d0deadbeefcafef00dfeed
COPY .env /app/.env
COPY --from=build /secret/key.pem /app/key.pem
EOF

if bash "$GUARD" "$TMP/Dockerfile" >/tmp/zt-red.log 2>&1; then
  echo "FAIL - guard PASSED on a Dockerfile with planted secrets (must FAIL)" >&2
  cat /tmp/zt-red.log >&2 || true
  exit 1
fi
echo "ok   - guard FAILS on planted ENV/ARG/COPY secrets (exit non-zero)"

# --- GREEN: real repo Dockerfiles must PASS -----------------------------------
if ! bash "$GUARD" >/tmp/zt-green.log 2>&1; then
  echo "FAIL - guard FAILED on the real repo Dockerfiles (must PASS)" >&2
  cat /tmp/zt-green.log >&2 || true
  exit 1
fi
echo "ok   - guard PASSES on the real backend + Edge Dockerfiles (exit zero)"

echo
echo "zero-trust guard self-test passed"
