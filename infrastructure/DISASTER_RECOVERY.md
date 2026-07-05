# Disaster Recovery — Second Brain

The human-readable runbook for catastrophic VPS loss. This is the source of
truth for the *why* and the exact command sequence; `infrastructure/bootstrap.sh`
automates the deterministic drudge work (ADR-0008). Execute once, under stress,
at a bad time — don't improvise.

## Current replication status

> **The Brain Replica is live (Litestream -> Cloudflare R2, ADR-0002 / #32).**
> A Litestream sidecar tails the Brain File's WAL second-by-second to an R2
> bucket, so the Recovery Point Objective is ~1 second. R2 lives in a failure
> domain fully isolated from the Hetzner VPS (off-provider, zero egress). A
> host-cron ntfy Health Push (ADR-0005 / #33) fires when replication stops or
> the volume nears capacity — the alert finds the operator, not the reverse.
> Both ship in the next deploy to `main` (GHA build -> GHCR -> VPS pull); until
> that deploy lands on the VPS, treat the VPS as the sole copy. The restore
> step below is real and matches `infrastructure/test/replica.sh --live` (a
> local MinIO round-trip that exercises the same replicate -> restore path).

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
- **Brain Replica + Health Push (ADR-0002 / ADR-0005, #32 / #33)**: implemented,
  shipping on the next deploy to `main`. After deploy: RPO ~1s to R2, and the
  host cron (`/etc/cron.d/second-brain-health-push`, every 5 min) pushes to ntfy
  on replication lag or volume exhaustion. `bootstrap.sh` now installs the
  litestream config + the health-push cron; `.env` must carry the R2 keys
  (`LITESTREAM_*`) and `NTFY_WEBHOOK_URL`.
- **Deferred / known gaps**:
  - No domain or HTTPS yet → HTTP on the raw IP. WebAuthn login needs a secure
    context, so auth is blocked until a domain + Caddy auto-HTTPS are wired
    (swap the per-host Caddyfile in at GHA build time).
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
key-only), lays down `/opt/second-brain/{docker-compose.yml,deploy.sh,
infrastructure/litestream.yml,infrastructure/health-push.sh}`, installs the
command-restricted deploy key from `infrastructure/keys/deploy.pub`, and places
the ntfy Health Push cron at `/etc/cron.d/second-brain-health-push` (every 5 min
— alerting once `NTFY_WEBHOOK_URL` is in `.env`).

### 2. Place the runtime secrets manually (ADR-0004)

GHA is blind to secrets — `.env` is placed by hand, once, over SSH.

```sh
scp infrastructure/.env root@<new-vps>:/opt/second-brain/infrastructure/.env
ssh root@<new-vps> 'chown deploy:deploy /opt/second-brain/infrastructure/.env && chmod 600 /opt/second-brain/infrastructure/.env'
```

### 3. Restore the Brain File from R2

The Brain Replica is a Litestream-managed copy of `/data/second_brain.db` in R2
(ADR-0002 / #32). Restore it into a fresh `sqlite_data` named volume BEFORE
bringing the stack up, so the backend opens the restored brain on first start.

```sh
# Run as root on the new VPS. --env-file injects the R2 creds from the .env you
# placed in step 2 (LITESTREAM_ACCESS_KEY_ID / LITESTREAM_SECRET_ACCESS_KEY);
# the mounted litestream.yml (installed by bootstrap) carries bucket + endpoint.
# `litestream restore` only runs if the output db does NOT exist — the fresh
# volume is empty, so this populates it. (Use -force to overwrite an existing
# brain in a partial-recovery scenario.)
docker run --rm \
  -v sqlite_data:/data \
  -v /opt/second-brain/infrastructure/litestream.yml:/etc/litestream.yml:ro \
  --env-file /opt/second-brain/infrastructure/.env \
  litestream/litestream:0.5.13 \
  restore -config /etc/litestream.yml /data/second_brain.db
```

Sanity-check the restore before continuing (optional): the restored file should
exist and be non-empty:

```sh
docker run --rm -v sqlite_data:/data alpine:3 \
  sh -c 'ls -l /data/second_brain.db && sqlite3 /data/second_brain.db "PRAGMA integrity_check"' 2>/dev/null \
  || echo "(alpine has no sqlite3 by default; skip — the backend will validate on open)"
```

If the bucket is empty (brand-new brain, never replicated), `litestream restore`
exits non-zero with "no matching backups found" — that is expected on the very
first deploy; skip this step and let the backend create a fresh empty brain.

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

An untested restore is a hope, not a strategy (ADR-0008). The trust contract of
a Second Brain is only as strong as the last confirmed restore.

- **Local replica round-trip (automated):** `bash infrastructure/test/replica.sh
  --live` exercises the replicate -> destroy -> restore -> verify path against a
  throwaway MinIO (R2 stand-in) on the dev machine — no VPS, no real R2. It
  proves the restore command shape and the Litestream round-trip end to end.
  Run it before any change to `litestream.yml` or the restore sequence.
- **Throwaway-VPS re-exercise (periodic, manual):** the procedure above MUST be
  re-exercised on a fresh throwaway Hetzner VPS periodically (e.g. quarterly, or
  after any Litestream/R2 config change) — provision a VPS, `bootstrap.sh`,
  place `.env` with real R2 creds, `litestream restore` from the live R2 bucket,
  `docker compose pull && up -d`, and confirm a known recent braindump is
  present. Destroy the VPS afterward. This is the only way to validate the full
  R2 path + credentials under realistic conditions; the local MinIO round-trip
  does not exercise real R2 or a real VPS.
