# Disaster Recovery — Second Brain

The human-readable runbook for catastrophic VPS loss. This is the source of
truth for the *why* and the exact command sequence; `infrastructure/bootstrap.sh`
automates the deterministic drudge work (ADR-0008). Execute once, under stress,
at a bad time — don't improvise.

## Current replication status

> **No offsite replica yet.** The Brain Replica (R2/Litestream, ADR-0002) and
> the ntfy Health Push (ADR-0005) are **not yet implemented** (deferred slices
> #32 / #33). Until they ship, **VPS loss = total data loss** — the Brain File
> lives only on the `sqlite_data` Docker volume on this single host. Treat the
> VPS as the sole copy. When slice #32 lands, the restore step below becomes
> real; update this runbook then.

## Current operational status

> Snapshot last confirmed: 2026-07-05. The deploy pipeline is **live and
> verified end-to-end** — a push to `main` triggers GHA (CI gate → build → GHCR
> push → command-restricted SSH → VPS `pull && up -d`). First fully-automated
> green run: commit `6c07f23`, GHA run `28720104235`.

- **VPS**: `89.58.14.42` (Debian 13, 4 GB RAM + 4 GB swap). Stack lives at
  `/opt/second-brain/` — `docker-compose.yml`, `deploy.sh`, `deploy.env`
  (SHA tags, GHA-written), `infrastructure/.env` (secrets, hand-placed).
  Health check: `curl http://89.58.14.42/api/health` →
  `{"db":true,"ok":true,"sqlite_vec":true}`.
- **Deploy key (manual rollback)**: `~/.ssh/sb_deploy_key` on the operator's
  machine (private, `chmod 600`). Public half is committed at
  `infrastructure/keys/deploy.pub` and installed on the VPS `deploy` user as a
  *command-restricted* authorized key (no shell, no pty — can only run
  `deploy.sh`). The only other private copy is the `SSH_DEPLOY_KEY` GitHub
  secret (for GHA). Rollback command is in the "Rollback" section below.
- **GHCR access**: images are pullable **anonymously** — the repo is public, so
  GHCR packages are public by default. No PAT on the VPS, no manual visibility
  flip was needed. (If the repo is ever taken private, the VPS will need a
  read-only `docker login` to GHCR.)
- **Firewall gotcha (load-bearing)**: `bootstrap.sh` applies an **INPUT-only**
  nftables ruleset, restarts Docker *after* the flush, and orders
  `docker.service After=nftables`. Do **not** add a `FORWARD`/`OUTPUT` flush to
  the firewall. Docker owns the `FORWARD` chain — container published ports
  traverse `FORWARD` after DNAT, not `INPUT`. Flushing `FORWARD` breaks
  `docker compose up -d` with `No chain/target/match by that name`. This was a
  real production fire on the first GHA deploy.
- **Deferred / known gaps**:
  - No domain or HTTPS yet → HTTP on the raw IP. WebAuthn login needs a secure
    context, so auth is blocked until a domain + Caddy auto-HTTPS are wired
    (swap the per-host Caddyfile in at GHA build time).
  - No Brain Replica yet (R2/Litestream, slice #32) → see "Current replication
    status" above; **VPS loss = total data loss today**.
  - No ntfy Health Push yet (slice #33).
  - `VITE_DEEPGRAM_API_KEY` unset → voice capture won't work until wired.

## Recovery procedure

Assume the VPS is gone (Hetzner host failure, accidental destroy, compromise).

### 1. Provision a fresh Debian 13 VPS and run bootstrap

```sh
# On the new VPS, as root:
git clone https://github.com/lifelonglearner94/second_brain.git
cd second_brain && bash infrastructure/bootstrap.sh
```

`bootstrap.sh` is idempotent. It: enables 4GB swap, verifies Docker, applies an
nftables firewall (22/80/443 open), creates the `deploy` user (docker group,
key-only), lays down `/opt/second-brain/{docker-compose.yml,deploy.sh}`, and
installs the command-restricted deploy key from `infrastructure/keys/deploy.pub`.

### 2. Place the runtime secrets manually (ADR-0004)

GHA is blind to secrets — `.env` is placed by hand, once, over SSH.

```sh
scp infrastructure/.env root@<new-vps>:/opt/second-brain/infrastructure/.env
ssh root@<new-vps> 'chown deploy:deploy /opt/second-brain/infrastructure/.env && chmod 600 /opt/second-brain/infrastructure/.env'
```

### 3. Restore the Brain File from R2  *(NOT YET AVAILABLE — slice #32)*

```sh
# Placeholder — Litestream sidecar + R2 Brain Replica are not implemented yet.
# When slice #32 ships, the sequence is roughly:
#   docker run --rm -v sqlite_data:/data \
#     litestream/litestream restore -o /data/second_brain.db s3://<bucket>/second_brain.db
# Skip this step today; there is nothing to restore from.
```

### 4. Bring the stack up

The first real deploy comes from a push to `main` (GHA builds, pushes GHCR,
SSHes in to pull + `up -d`). To bring the stack up immediately without waiting
for CI, on the VPS as `deploy`:

```sh
cd /opt/second-brain
# Set deploy.env to the last known-good SHA tag (find it via GHCR or `docker images`),
# then:
docker compose --env-file deploy.env -f docker-compose.yml pull
docker compose --env-file deploy.env -f docker-compose.yml up -d
```

Verify: `curl http://<vps>/api/health` should return `{"ok":...,"db":...}`.

## Rollback (bad deploy, no VPS loss)

SHA-pinned tags make this a 30-second op (ADR-0007):

```sh
ssh deploy@<vps>  # forced command reads deploy.env from stdin
# pipe the previous known-good SHA:
printf 'REGISTRY=ghcr.io/lifelonglearner94/\nEDGE_TAG=sha-<good>\nBACKEND_TAG=sha-<good>\n' | ssh deploy@<vps>
```

The previous image is cached on the VPS; `pull` is a no-op and `up -d` reverts.
Find prior SHAs in the GHCR package history or the Actions deploy logs.

## Testing this procedure

An untested restore is a hope, not a strategy (ADR-0008). Once slice #32 lands,
exercise this runbook on a throwaway VPS periodically — the trust contract of a
Second Brain is only as strong as the last confirmed restore.
