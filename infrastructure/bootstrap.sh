#!/usr/bin/env bash
# Bootstrap a fresh Debian VPS for the Second Brain stack (edge + backend +
# the Litestream Brain Replica sidecar). Idempotent: safe to re-run. Automates
# the deterministic drudge work from ADR-0008 (infrastructure/DISASTER_RECOVERY.md
# is the human source of truth for the why and the exact command sequence).
#
# Run from a checkout of the repo on the VPS:
#   git clone https://github.com/lifelonglearner94/second_brain.git
#   cd second_brain && bash infrastructure/bootstrap.sh
#
# Handles everything reproducible: swap, Docker, nftables firewall, the deploy
# user + command-restricted key, the stack files (docker-compose.yml,
# litestream.yml, deploy.sh, health-push.sh), and the ntfy Health Push cron
# (ADR-0005, #33). deploy.sh re-syncs the three non-secret config files
# (docker-compose.yml, litestream.yml, health-push.sh) from the public repo on
# every deploy (ADR-0010); bootstrap installs them once to bootstrap the sync.
# Does NOT handle secrets - .env (R2 creds + NTFY_WEBHOOK_URL + Gemini key) is
# placed manually (ADR-0004). The Brain Replica itself runs as the litestream
# sidecar in docker-compose.yml; bootstrap only installs its config file.
set -euo pipefail

DEPLOY_USER="deploy"
INSTALL_DIR="/opt/second-brain"
GHCR_OWNER="lifelonglearner94"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo ">>> bootstrap: repo root = $REPO_ROOT"
[[ -f "$REPO_ROOT/docker-compose.yml" ]] || { echo "FAIL: run from the repo root (docker-compose.yml not found at $REPO_ROOT)"; exit 1; }

# --- 1. Swap (safety net - the VPS has 4GB RAM and zero swap) ----------------
if [[ "$(swapon --show --noheadings 2>/dev/null | wc -l)" -eq 0 ]]; then
  echo ">>> creating 4GB swapfile at /swapfile"
  fallocate -l 4G /swapfile 2>/dev/null || dd if=/dev/zero of=/swapfile bs=1M count=4096
  chmod 600 /swapfile
  mkswap /swapfile >/dev/null
  swapon /swapfile
  grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
  echo ">>> swap enabled (4GB)"
else
  echo ">>> swap already present, skipping"
fi
# Swappiness is set unconditionally (outside the create-swap branch) so a
# re-bootstrap with swap already present still writes the drop-in. The previous
# version only wrote it inside the if-branch, so a re-run left the drop-in
# missing and vm.swappiness did not survive reboot. Debian 13 has no
# /etc/sysctl.conf, so the drop-in is the persistence mechanism.
if command -v sysctl >/dev/null 2>&1; then sysctl -w vm.swappiness=10 >/dev/null; fi
echo 'vm.swappiness=10' > /etc/sysctl.d/99-second-brain.conf
echo ">>> swappiness=10 (runtime + /etc/sysctl.d/99-second-brain.conf)"

# --- 2. Docker ---------------------------------------------------------------
if ! command -v docker >/dev/null 2>&1; then
  echo ">>> installing Docker"
  curl -fsSL https://get.docker.com | sh
else
  echo ">>> docker present: $(docker --version)"
fi
if ! docker compose version >/dev/null 2>&1; then
  echo "FAIL: docker compose plugin missing"; exit 1
fi

# --- 3. Firewall (nftables INPUT-only - Docker owns FORWARD) ----------------
# Docker (via iptables-nft) creates and manages its own FORWARD + DOCKER* chains
# in the `ip filter` table for bridge networking. A `flush ruleset` + our own
# FORWARD policy would wipe those chains and break `docker compose up` ("No
# chain/target/match by that name" on DOCKER-FORWARD). So we manage INPUT only
# - container-published ports traverse FORWARD (after DNAT), not INPUT, so a
# drop-INPUT firewall does not block the Edge's :80. Docker is restarted after
# the flush so it recreates its chains over the clean base, and docker.service
# is ordered After nftables so a reboot flushes before Docker starts.
if command -v nft >/dev/null 2>&1; then
  echo ">>> configuring nftables firewall (INPUT-only: 22/80/443 open, rest drop)"
  cat > /etc/nftables.conf <<'NFT'
#!/usr/sbin/nft -f
flush ruleset
table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;
        iif "lo" accept
        ct state established,related accept
        tcp dport 22 accept
        tcp dport 80 accept
        tcp dport 443 accept
        icmp type echo-request accept
        iif != "lo" counter drop
    }
}
NFT
  if nft -c -f /etc/nftables.conf 2>/dev/null; then
    nft -f /etc/nftables.conf
    systemctl enable --now nftables 2>/dev/null || true
    # On boot, nftables must apply its flush BEFORE docker.service recreates
    # the DOCKER chains, otherwise the flush wipes chains Docker already made.
    mkdir -p /etc/systemd/system/docker.service.d
    cat > /etc/systemd/system/docker.service.d/10-after-nftables.conf <<'UNIT'
[Unit]
After=nftables.service
Wants=nftables.service
UNIT
    systemctl daemon-reload
    # Recreate Docker's chains over the freshly-flushed base (no-op if absent).
    if systemctl is-active --quiet docker; then
      systemctl restart docker
    fi
    echo ">>> firewall active (INPUT-only); docker restarted to rebuild its chains"
  else
    echo ">>> WARNING: nftables config failed syntax check; firewall NOT applied (host left open)"
  fi
else
  echo ">>> nftables not available; skipping firewall"
fi

# --- 4. Deploy user (docker group, password locked, key-only) ----------------
if ! id "$DEPLOY_USER" >/dev/null 2>&1; then
  echo ">>> creating user $DEPLOY_USER"
  useradd -m -s /bin/bash "$DEPLOY_USER"
  passwd -l "$DEPLOY_USER" >/dev/null 2>&1 || true
fi
getent group docker >/dev/null 2>&1 && usermod -aG docker "$DEPLOY_USER"

# --- 5. Install dir + stack files --------------------------------------------
# Ownership split (ADR-0010): the three sync-eligible config files are owned by
# $DEPLOY_USER so deploy.sh (which runs as $DEPLOY_USER via the forced command)
# can `cp` over them in place on every deploy. deploy.sh ITSELF stays root-owned
# and $INSTALL_DIR stays root-owned - so the deploy key (no shell, forced
# command) cannot create new files in $INSTALL_DIR and cannot replace the
# deploy.sh gate that validates its input. Overwriting an existing file you own
# needs write perm on the FILE only (not the dir), so deploy can update these
# deploy-owned files but not delete/rename them or touch deploy.sh.
echo ">>> installing stack files to $INSTALL_DIR"
install -d -o root          -g root          -m 755 "$INSTALL_DIR"
install -d -o "$DEPLOY_USER" -g "$DEPLOY_USER" -m 700 "$INSTALL_DIR/infrastructure"
# Sync-eligible (deploy-owned): deploy.sh overwrites these from the public repo
# at the deployed SHA on every deploy (ADR-0010).
install -m 644 -o "$DEPLOY_USER" -g "$DEPLOY_USER" "$REPO_ROOT/docker-compose.yml"        "$INSTALL_DIR/docker-compose.yml"
# Brain Replica config (ADR-0002, #32): mounted read-only by the litestream
# sidecar via docker-compose's ./infrastructure/litestream.yml bind mount.
install -m 644 -o "$DEPLOY_USER" -g "$DEPLOY_USER" "$REPO_ROOT/infrastructure/litestream.yml" "$INSTALL_DIR/infrastructure/litestream.yml"
# Health Push script (ADR-0005, #33): run by the /etc/cron.d entry below; reads
# NTFY_WEBHOOK_URL from the manually-placed infrastructure/.env (ADR-0004).
install -m 755 -o "$DEPLOY_USER" -g "$DEPLOY_USER" "$REPO_ROOT/infrastructure/health-push.sh" "$INSTALL_DIR/infrastructure/health-push.sh"
# The gate: root-owned, NOT deploy-writable. deploy.sh validates stdin against
# the ADR-0007 whitelist before writing anything - if deploy could replace it,
# a leaked key could neuter the validation. Updated manually like .env (one-time
# scp), not via the sync it gates.
install -m 755 -o root -g root "$REPO_ROOT/infrastructure/deploy.sh"  "$INSTALL_DIR/deploy.sh"
touch "$INSTALL_DIR/deploy.log"
chown "$DEPLOY_USER":"$DEPLOY_USER" "$INSTALL_DIR/deploy.log"
chmod 644 "$INSTALL_DIR/deploy.log"

# Placeholder deploy.env so `docker compose config` resolves before the first
# GHA deploy writes a real SHA tag (ADR-0007). Never pulled - :latest has no
# GHCR image; the first real deploy overwrites this with a SHA tag.
if [[ ! -f "$INSTALL_DIR/deploy.env" ]]; then
  cat > "$INSTALL_DIR/deploy.env" <<EOF
# Placeholder until first GHA deploy (ADR-0007). Overwritten on every deploy.
REGISTRY=ghcr.io/${GHCR_OWNER}/
EDGE_TAG=latest
BACKEND_TAG=latest
EOF
  chown "$DEPLOY_USER":"$DEPLOY_USER" "$INSTALL_DIR/deploy.env"
  chmod 600 "$INSTALL_DIR/deploy.env"
fi

# --- 6. Health Push cron (ADR-0005, #33) -------------------------------------
# Zero-RAM survival check: a /etc/cron.d entry runs health-push.sh every 5 min
# to push to ntfy when the Brain Replica stops replicating or the volume nears
# capacity. The script ships no secrets (NTFY_WEBHOOK_URL comes from .env).
install -d -o root -g root -m 0755 /var/lib/second-brain           # health-push state dir
if [[ -f "$REPO_ROOT/infrastructure/health-push.cron" ]]; then
  install -m 0644 -o root -g root "$REPO_ROOT/infrastructure/health-push.cron" /etc/cron.d/second-brain-health-push
  # /etc/cron.d requires a cron daemon. Install + enable one if absent.
  if ! command -v crontab >/dev/null 2>&1; then
    echo ">>> installing cron (for the Health Push /etc/cron.d entry)"
    apt-get update -qq && apt-get install -y -qq cron
  fi
  systemctl enable --now cron >/dev/null 2>&1 || true
  echo ">>> health-push cron installed (/etc/cron.d/second-brain-health-push, every 5 min)"
else
  echo ">>> WARNING: infrastructure/health-push.cron missing - Health Push cron NOT installed"
fi

# --- 7. Command-restricted deploy SSH key (ADR-0003) ------------------------
PUBKEY_FILE="$REPO_ROOT/infrastructure/keys/deploy.pub"
ssh_dir="/home/$DEPLOY_USER/.ssh"
install -d -o "$DEPLOY_USER" -g "$DEPLOY_USER" -m 700 "$ssh_dir"
auth="$ssh_dir/authorized_keys"
touch "$auth"; chown "$DEPLOY_USER":"$DEPLOY_USER" "$auth"; chmod 600 "$auth"
if [[ -f "$PUBKEY_FILE" && -s "$PUBKEY_FILE" ]]; then
  pubkey="$(cat "$PUBKEY_FILE")"
  # Forced command = deploy.sh; no pty, no forwarding. The restriction IS the
  # security model (ADR-0003): a leaked key can only pull + restart the stack.
  entry="command=\"$INSTALL_DIR/deploy.sh\",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding $pubkey"
  if ! grep -qF "$pubkey" "$auth"; then
    printf '%s\n' "$entry" >> "$auth"
    echo ">>> installed command-restricted deploy key for $DEPLOY_USER"
  else
    echo ">>> deploy key already present in authorized_keys"
  fi
else
  echo ">>> WARNING: $PUBKEY_FILE missing - deploy key NOT installed (GHA deploys will fail)"
fi

# --- sshd: disable password auth for the deploy user (key only) --------------
if ! grep -q "Match User $DEPLOY_USER" /etc/ssh/sshd_config 2>/dev/null; then
  {
    printf '\n# Second Brain deploy user (ADR-0003) - key only, no password.\n'
    printf 'Match User %s\n' "$DEPLOY_USER"
    printf '    PasswordAuthentication no\n'
  } >> /etc/ssh/sshd_config
  systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true
fi

echo
echo ">>> bootstrap complete."
echo ">>> NEXT (manual - secrets, ADR-0004):"
echo ">>>   1. Fill infrastructure/.env on your machine: GEMINI_API_KEY + the"
echo ">>>      Brain Replica R2 keys (LITESTREAM_*) + NTFY_WEBHOOK_URL."
echo ">>>   2. scp infrastructure/.env root@<vps>:$INSTALL_DIR/infrastructure/.env"
echo ">>>   3. ssh root@<vps> 'chown $DEPLOY_USER:$DEPLOY_USER $INSTALL_DIR/infrastructure/.env; chmod 600 $INSTALL_DIR/infrastructure/.env'"
echo ">>>   4. Push to main - GHA builds, pushes GHCR, SSHes here to pull + up."
echo ">>> The Health Push cron (/etc/cron.d/second-brain-health-push) is already"
echo ">>> installed and will alert via ntfy once NTFY_WEBHOOK_URL is in .env."
echo ">>> To RESTORE from R2 after a VPS loss, see DISASTER_RECOVERY.md."
