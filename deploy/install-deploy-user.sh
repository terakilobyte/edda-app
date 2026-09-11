#!/usr/bin/env bash
# The CI deploy identity on the box: user `deploy`, one key, one command.
# Run as root from the deploy kit directory (bootstrap.sh calls it; it
# is also safe to run alone on a live box — nothing here restarts a
# service). Idempotent: every file is rewritten from this kit each run,
# so the box's state is exactly what the repository says.
#
# What the key can do, and nothing else (deploy/edda-deploy):
#   - rsync into /var/lib/edda-deploy/inbox (rrsync, write-only)
#   - `apply api|app|site|dashboards|routing` → sudo edda-apply, each verb named
#     exactly in sudoers; edda-apply is root-owned and not writable by
#     deploy, so the rule cannot be widened from the deploy side
#   - `status`
# `restrict` on the key: no pty, no agent/port/X11 forwarding, no rc files.
# The key's public half is deploy/deploy-key.pub; the private half lives
# only in 1Password and the DEPLOY_SSH_KEY repository secret (README,
# "Release = push a tag").
set -euo pipefail

KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOME_DIR=/var/lib/edda-deploy

echo "== deploy user =="
apt-get install -y -qq rsync curl >/dev/null
[[ -x /usr/bin/rrsync ]] || { echo "rrsync missing: rsync >= 3.2.4 ships it as /usr/bin/rrsync" >&2; exit 1; }
id -u deploy >/dev/null 2>&1 || useradd --system --home-dir "$HOME_DIR" --create-home --shell /bin/bash deploy
mkdir -p "$HOME_DIR/inbox"
chown root:root "$HOME_DIR"
chmod 755 "$HOME_DIR"
chown deploy:deploy "$HOME_DIR/inbox"
chmod 700 "$HOME_DIR/inbox"

echo "== authorized key (forced command, restricted) =="
[[ -s "$KIT/deploy-key.pub" ]] || { echo "$KIT/deploy-key.pub is missing" >&2; exit 1; }
[[ $(wc -l < "$KIT/deploy-key.pub") -eq 1 ]] || { echo "deploy-key.pub must hold exactly one key" >&2; exit 1; }
# ~/.ssh and authorized_keys are root-owned: sshd's StrictModes accepts
# root or the user as owner, and the deploy side then cannot add keys.
mkdir -p "$HOME_DIR/.ssh"
chown root:root "$HOME_DIR/.ssh"
chmod 755 "$HOME_DIR/.ssh"
printf 'restrict,command="/usr/local/bin/edda-deploy" %s\n' "$(cat "$KIT/deploy-key.pub")" > "$HOME_DIR/.ssh/authorized_keys"
chown root:root "$HOME_DIR/.ssh/authorized_keys"
chmod 644 "$HOME_DIR/.ssh/authorized_keys"

echo "== scripts + sudoers =="
install -o root -g root -m 755 "$KIT/edda-deploy" /usr/local/bin/edda-deploy
install -o root -g root -m 755 "$KIT/edda-apply" /usr/local/sbin/edda-apply
tmp=$(mktemp)
cat > "$tmp" <<'SUDOERS'
# EDDA CI deploy: user deploy may run exactly these four commands as root.
Defaults!/usr/local/sbin/edda-apply !requiretty
deploy ALL=(root) NOPASSWD: /usr/local/sbin/edda-apply api, /usr/local/sbin/edda-apply app, /usr/local/sbin/edda-apply site, /usr/local/sbin/edda-apply dashboards, /usr/local/sbin/edda-apply routing
SUDOERS
visudo -cf "$tmp" >/dev/null
install -o root -g root -m 440 "$tmp" /etc/sudoers.d/edda-deploy
rm -f "$tmp"
sshd -t

echo "== installed =="
echo "deploy key: $(ssh-keygen -lf "$KIT/deploy-key.pub")"
echo
echo "root's authorized_keys (revoke any earlier CI key by hand; only the"
echo "maintainer's own key belongs here now):"
ssh-keygen -lf /root/.ssh/authorized_keys 2>/dev/null || echo "  (none)"
