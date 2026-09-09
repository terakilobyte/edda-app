#!/usr/bin/env bash
# EDDA server bootstrap: run once as root on a fresh Ubuntu 24.04 box,
# from the directory holding this kit (push-api.ps1 does exactly that).
# Idempotent where it matters: re-running repairs rather than duplicates.
set -euo pipefail

DOMAIN="api.edda-app.com"

echo "== packages (and catch up on security updates — audit M2: a fresh
== image starts a day behind; update alone never upgrades) =="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get upgrade -y -qq
apt-get install -y -qq postgresql ufw unattended-upgrades zstd curl gnupg tmux fail2ban

echo "== automatic security updates (with reboots at 04:30 UTC) =="
cat > /etc/apt/apt.conf.d/51edda-unattended <<'CFG'
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";
CFG
systemctl enable --now unattended-upgrades

echo "== fail2ban (sshd jail, defaults) =="
systemctl enable --now fail2ban

echo "== caddy (official repo) =="
if ! command -v caddy >/dev/null; then
    curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key |
        gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt |
        tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
    apt-get update -qq && apt-get install -y -qq caddy
fi

echo "== firewall + ssh hardening =="
ufw default deny incoming
ufw default allow outgoing
ufw allow OpenSSH
ufw allow 80/tcp
ufw allow 443/tcp
# Caddy advertises h3; the advertisement must be honest (audit L3).
ufw allow 443/udp
ufw --force enable
# A drop-in, never sed: Ubuntu 24.04's stock sshd_config carries no
# PasswordAuthentication line (the permissive value is a compiled
# default), so a sed matches nothing and the box stays password-enabled
# while looking hardened (field catch by review, 2026-09-04, on the
# live box). Include + first-value-wins makes 00- deterministic.
cat > /etc/ssh/sshd_config.d/00-edda-hardening.conf <<'SSHD'
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
X11Forwarding no
SSHD
sshd -t
systemctl reload ssh

echo "== swap (audit H2: the weekly publish peaks ~8 GB; without swap the OOM killer is the only mediator) =="
if ! swapon --show | grep -q /swapfile; then
    fallocate -l 8G /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi
echo 'vm.swappiness=10' > /etc/sysctl.d/99-edda.conf
sysctl -p /etc/sysctl.d/99-edda.conf

echo "== journald cap =="
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nSystemMaxUse=1G\n' > /etc/systemd/journald.conf.d/edda.conf
systemctl restart systemd-journald

echo "== user + directories =="
id -u edda >/dev/null 2>&1 || useradd --system --home /var/lib/edda --shell /usr/sbin/nologin edda
mkdir -p /var/lib/edda/{artifacts,dumps,backups} /etc/edda
chown -R edda:edda /var/lib/edda
# Normalize permissions every run: content seeded through a Windows
# mount arrives 777 (audit H1 — the served MANIFEST was world-writable,
# a supply-chain hole for any local account). Dirs 755, files 644.
chmod -R u=rwX,g=rX,o=rX /var/lib/edda/artifacts

echo "== postgres tuning (audit H1: stock 128 MB shared_buffers under a
== 23 GB database; 32 checkpoint warnings on restore night) =="
mkdir -p /etc/postgresql/16/main/conf.d
cat > /etc/postgresql/16/main/conf.d/edda.conf <<'PGTUNE'
shared_buffers = 4GB
effective_cache_size = 12GB
maintenance_work_mem = 1GB
work_mem = 32MB
max_wal_size = 8GB
random_page_cost = 1.1
effective_io_concurrency = 200
PGTUNE
systemctl restart postgresql

echo "== postgres =="
if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='edda'" | grep -q 1; then
    # openssl, not tr</dev/urandom|head: under pipefail the head-close
    # SIGPIPEs tr and set -e kills the whole bootstrap at this line
    # (field case: first bring-up died exactly here).
    PGPASS=$(openssl rand -hex 16)
    sudo -u postgres psql -c "CREATE ROLE edda LOGIN PASSWORD '${PGPASS}'"
    sudo -u postgres createdb -O edda edda
    cat > /etc/edda/api.env <<ENV
DATABASE_URL=postgres://edda:${PGPASS}@127.0.0.1:5432/edda
EDDA_API_ARTIFACT_DIR=/var/lib/edda/artifacts
EDDA_API_BIND=127.0.0.1:8787
ENV
    chmod 600 /etc/edda/api.env
    chown root:root /etc/edda/api.env
fi

echo "== caddy config =="
install -m 644 Caddyfile /etc/caddy/Caddyfile
# restart, never reload: with `admin off` the reload path (which talks
# to the admin API) always fails (review field finding, 2026-09-04).
systemctl restart caddy

echo "== systemd units =="
install -m 644 edda-api.service edda-eddn.service edda-daily.service edda-daily.timer \
    edda-weekly.service edda-weekly.timer edda-backup.service edda-backup.timer \
    /etc/systemd/system/
install -m 755 edda-daily.sh edda-weekly.sh edda-backup.sh /usr/local/bin/
systemctl daemon-reload
# The feed is edda-eddn.service's; serve must not run a second copy.
grep -q '^EDDA_API_EDDN_IN_SERVE=' /etc/edda/api.env || echo 'EDDA_API_EDDN_IN_SERVE=false' >> /etc/edda/api.env
systemctl enable --now edda-eddn.service
systemctl enable --now edda-api.service
systemctl enable --now edda-daily.timer edda-weekly.timer edda-backup.timer

echo "== done =="
systemctl --no-pager --lines 3 status edda-api.service || true
echo "next: seed the database (README step 3), then https://${DOMAIN}/healthz"
