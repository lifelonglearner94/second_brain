#!/usr/bin/env bash
# Zero-Trust Image guard (ADR-0004, issue #30).
#
# Asserts that no Dockerfile in the repo bakes a secret literal into an ENV,
# ARG, or COPY directive — so images stay public-safe artifacts: if a GHCR
# image leaked to the public internet, only compiled code would be exposed.
#
#   bash infrastructure/test/zero-trust.sh            # scan every Dockerfile in the repo
#   bash infrastructure/test/zero-trust.sh PATH...     # scan the given file(s)/dir(s)
#
# Exit 0 = zero-trust (no secret literals found); exit 1 = a directive bakes a
# secret. GHA-blind: this guard never reads infrastructure/.env — it only
# scans Dockerfiles (and may read .env.example, which carries no values).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# Build the list of Dockerfiles to scan. Explicit args win; otherwise glob the
# whole repo (pruning build/output dirs), matching any `Dockerfile` or
# `Dockerfile.*` so future images are covered automatically.
if [[ $# -gt 0 ]]; then
  DOCKERFILES=("$@")
else
  DOCKERFILES=()
  while IFS= read -r -d '' f; do
    DOCKERFILES+=("$f")
  done < <(find . \
      -type d \( -name .git -o -name target -o -name node_modules -o -name .svelte-kit \) -prune \
      -o -type f \( -name 'Dockerfile' -o -name 'Dockerfile.*' \) -print0)
fi

if [[ ${#DOCKERFILES[@]} -eq 0 ]]; then
  echo "ok   - no Dockerfiles to scan (zero-trust vacuously holds)" >&2
  exit 0
fi

python3 - "${DOCKERFILES[@]}" <<'PY'
import os, re, sys

# Env/ARG key segments (upper-snake-case) that mark a variable as a secret.
SECRET_WORD_SEGS = {"KEY", "SECRET", "TOKEN", "PASSWORD", "CREDENTIAL", "PASSPHRASE"}

# A value is a placeholder (no literal baked in) if it is empty or `$`-driven.
def is_placeholder(v):
    v = v.strip()
    return v == "" or v.startswith("$")

# Value-only heuristic: a long, dense, base64/hex/url-safe string with digits
# or mixed case looks like a baked credential even under a benign name.
def looks_like_secret_literal(v):
    v = v.strip().strip('"').strip("'")
    if len(v) < 20:
        return False
    if not re.fullmatch(r'[A-Za-z0-9+/=_-]+', v):
        return False  # has dots/slashes/colons/spaces -> path, url, or prose
    has_digit = any(c.isdigit() for c in v)
    has_lower = any(c.islower() for c in v)
    has_upper = any(c.isupper() for c in v)
    return has_digit or (has_lower and has_upper)

def key_is_secret(name):
    segs = re.split(r'[_\-]', name.upper())
    return any(s in SECRET_WORD_SEGS for s in segs)

# COPY basenames that are secret-bearing files. .env.example and friends are
# explicitly safe (blank values, committed).
SAFE_ENV_EXAMPLES = {s.lower() for s in
                     {".env.example", ".env.sample", ".env.template", ".env.dist", ".env.defaults"}}
SECRET_FILE_RE = re.compile(
    r'(\.env$'                           # the real .env (with secret values)
    r'|\.env\.[a-z][a-z0-9_-]*$'         # .env.PRODUCTION / .env.local (real values)
    r'|^id_(rsa|ed25519|ecdsa)$'
    r'|\.(pem|key|p12|pfx|keystore|jks)$'
    r'|(secret|credential|password|token|apikey).*\.[a-z]+$)',
    re.IGNORECASE,
)
def copy_path_is_secret(p):
    base = os.path.basename(p.rstrip("/")).lower()
    if base in SAFE_ENV_EXAMPLES:
        return False
    return bool(SECRET_FILE_RE.search(base))

# Join Dockerfile backslash line continuations into single logical lines,
# tracking the original line number of the first physical line.
def logical_lines(text):
    out, lineno, buf = [], 0, ""
    for i, raw in enumerate(text.splitlines(), 1):
        line = raw.rstrip("\r")
        stripped = line.strip()
        if stripped.startswith("#") or stripped == "":
            if buf:
                out.append((lineno, buf)); buf = ""
            continue
        if buf:
            buf += " " + stripped
        else:
            lineno = i; buf = stripped
        if buf.endswith("\\"):
            buf = buf[:-1].rstrip()
            continue
        if buf:
            out.append((lineno, buf)); buf = ""
    if buf:
        out.append((lineno, buf))
    return out

def parse_env_tokens(rest):
    # Modern `ENV K=v K2=v2 ...` if a '=' appears before any whitespace in the
    # first token; legacy `ENV K v...` otherwise.
    first = rest.split(None, 1)[0] if rest else ""
    if "=" in first:
        toks = rest.split()
        pairs = []
        for t in toks:
            k, _, v = t.partition("=")
            pairs.append((k, v))
        return pairs
    k, _, v = rest.partition(" ")
    return [(k, v)]

def violations_for(path):
    found = []
    try:
        text = open(path, "r", encoding="utf-8").read()
    except OSError as e:
        return [f"{path}: cannot read: {e}"]
    for lineno, line in logical_lines(text):
        parts = line.split(None, 1)
        if not parts:
            continue
        directive = parts[0].upper()
        rest = parts[1] if len(parts) > 1 else ""
        if directive == "ENV":
            for k, v in parse_env_tokens(rest):
                if is_placeholder(v):
                    continue
                if key_is_secret(k):
                    found.append(f"{path}:{lineno}: ENV {k}=<secret-literal> (secret-named key baked in)")
                elif looks_like_secret_literal(v):
                    found.append(f"{path}:{lineno}: ENV {k}=<long dense literal> (looks like a credential)")
        elif directive == "ARG":
            tok = rest.split(None, 1)[0] if rest else ""
            if "=" in tok:
                k, _, v = tok.partition("=")
                if not is_placeholder(v):
                    if key_is_secret(k):
                        found.append(f"{path}:{lineno}: ARG {k}=<secret-literal> (secret-named key baked in)")
                    elif looks_like_secret_literal(v):
                        found.append(f"{path}:{lineno}: ARG {k}=<long dense literal> (looks like a credential)")
        elif directive == "COPY":
            args = [a for a in rest.split() if not a.startswith("--")]
            for a in args:
                if copy_path_is_secret(a):
                    found.append(f"{path}:{lineno}: COPY <{a}> (secret-bearing file baked into image)")
    return found

files = sys.argv[1:]
all_viol = []
for f in files:
    all_viol.extend(violations_for(f))

if all_viol:
    print(f"FAIL - Zero-Trust Image violation(s) in {len(files)} Dockerfile(s):", file=sys.stderr)
    for v in all_viol:
        print(f"  {v}", file=sys.stderr)
    print("  A secret literal must never touch a Dockerfile ENV/ARG/COPY (ADR-0004).", file=sys.stderr)
    sys.exit(1)

print(f"ok   - {len(files)} Dockerfile(s) scanned, no secret literals in ENV/ARG/COPY (Zero-Trust, ADR-0004)")
sys.exit(0)
PY
