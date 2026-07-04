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
