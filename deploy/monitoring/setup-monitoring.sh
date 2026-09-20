#!/usr/bin/env bash
# Telemetry infrastructure (maintainer make-it-so, 2026-09-05): VictoriaMetrics
# single-node + Grafana OSS, BOTH loopback-only — the surveillance law's
# corollary is no exposed dashboards; the maintainer reaches them through an
# ssh tunnel (deploy/README.md). Run as root from this directory.
set -euo pipefail

VM_VERSION="v1.151.0"

echo "== victoria-metrics ${VM_VERSION} =="
if [ ! -x /usr/local/bin/victoria-metrics ]; then
    curl -fsSL -o /tmp/vm.tar.gz \
        "https://github.com/VictoriaMetrics/VictoriaMetrics/releases/download/${VM_VERSION}/victoria-metrics-linux-amd64-${VM_VERSION}.tar.gz"
    tar -xzf /tmp/vm.tar.gz -C /tmp
    install -m 755 /tmp/victoria-metrics-prod /usr/local/bin/victoria-metrics
    rm -f /tmp/vm.tar.gz /tmp/victoria-metrics-prod
fi
mkdir -p /var/lib/edda/metrics
chown -R edda:edda /var/lib/edda/metrics
install -m 644 vm-scrape.yml /etc/edda/vm-scrape.yml
install -m 644 edda-vm.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now edda-vm.service
# A re-run must pick up new scrape jobs: SIGHUP (ExecReload) re-reads
# -promscrape.config; a fresh install just starts.
systemctl reload-or-restart edda-vm.service

echo "== host + postgres exporters (loopback only) =="
# The Host Health dashboard reads node_* and the Server dashboard's
# Postgres row reads pg_*; neither had an exporter until the 2026-09-20
# no-data audit (docs/benches/2026-09-20-dashboard-no-data-audit.csv):
# every one of those panels sat on "No data" because nothing scraped
# them. Both are Ubuntu 24.04 universe packages. The package postinst
# starts each exporter on 0.0.0.0 with the stock defaults; the defaults
# below rebind them to loopback and the restart makes it so. The gap is
# a few seconds behind ufw's default-deny (bootstrap.sh), so never
# reachable, but the verify block at the end still checks the binds.
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq prometheus-node-exporter prometheus-postgres-exporter
cat > /etc/default/prometheus-node-exporter <<'ENV'
ARGS="--web.listen-address=127.0.0.1:9100"
ENV
# The postgres exporter connects as the `prometheus` role over the unix
# socket (peer auth, no password to store); pg_monitor covers
# pg_stat_database and pg_database_size — the panels' entire need.
if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='prometheus'" | grep -q 1; then
    sudo -u postgres psql -q -c "CREATE ROLE prometheus LOGIN"
fi
sudo -u postgres psql -q -c "GRANT pg_monitor TO prometheus"
cat > /etc/default/prometheus-postgres-exporter <<'ENV'
DATA_SOURCE_NAME='user=prometheus host=/run/postgresql dbname=edda'
ARGS="--web.listen-address=127.0.0.1:9187"
ENV
systemctl enable --now prometheus-node-exporter prometheus-postgres-exporter
systemctl restart prometheus-node-exporter prometheus-postgres-exporter

echo "== grafana oss (official repo) =="
if ! command -v grafana-server >/dev/null; then
    mkdir -p /etc/apt/keyrings
    curl -fsSL https://apt.grafana.com/gpg.key | gpg --dearmor -o /etc/apt/keyrings/grafana.gpg
    echo "deb [signed-by=/etc/apt/keyrings/grafana.gpg] https://apt.grafana.com stable main" \
        > /etc/apt/sources.list.d/grafana.list
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq grafana
fi

echo "== grafana config: loopback only, provisioned =="
# Loopback bind + no anonymous access + no phoning home.
cat > /etc/grafana/grafana.ini <<'INI'
[server]
http_addr = 127.0.0.1
http_port = 3000
[analytics]
reporting_enabled = false
check_for_updates = false
[users]
allow_sign_up = false
INI
if [ ! -f /etc/edda/grafana-admin ]; then
    GPASS=$(openssl rand -hex 12)
    printf 'admin\n%s\n' "$GPASS" > /etc/edda/grafana-admin
    chmod 600 /etc/edda/grafana-admin
    APPLY_ADMIN_PASSWORD=1
fi

echo "== read-only postgres role for the feedback browse =="
if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='grafana_ro'" | grep -q 1; then
    ROPASS=$(openssl rand -hex 12)
    sudo -u postgres psql -q -c "CREATE ROLE grafana_ro LOGIN PASSWORD '${ROPASS}'"
    printf 'postgres://grafana_ro:%s@127.0.0.1:5432/edda\n' "$ROPASS" > /etc/edda/grafana-pg
    chmod 600 /etc/edda/grafana-pg
    ROPG="$ROPASS"
else
    ROPG=$(sed -E 's|.*grafana_ro:([^@]*)@.*|\1|' /etc/edda/grafana-pg)
fi
# Grants run EVERY time, outside the create branch (field case
# 2026-09-05: the first run granted before the feedback migration had
# created the table, and a role-exists rerun skipped grants entirely —
# the dashboard read nothing until review re-granted live). USAGE on
# the schema is required alongside table SELECT.
sudo -u postgres psql -q -d edda -c "GRANT USAGE ON SCHEMA public TO grafana_ro" || true
sudo -u postgres psql -q -d edda -c "GRANT SELECT ON feedback TO grafana_ro" || true

mkdir -p /etc/grafana/provisioning/datasources /etc/grafana/provisioning/dashboards /var/lib/edda/dashboards
sed "s/__PG_PASSWORD__/${ROPG}/" grafana-datasources.yml > /etc/grafana/provisioning/datasources/edda.yaml
chmod 640 /etc/grafana/provisioning/datasources/edda.yaml
chown root:grafana /etc/grafana/provisioning/datasources/edda.yaml
install -m 644 grafana-dashboards.yml /etc/grafana/provisioning/dashboards/edda.yaml
install -m 644 dashboards/*.json /var/lib/edda/dashboards/
systemctl enable --now grafana-server
if [ "${APPLY_ADMIN_PASSWORD:-0}" = "1" ]; then
    # Via the HTTP API, never grafana-cli: the CLI writes the DB directly
    # and SILENTLY no-ops on package installs without --homepath/--config
    # (field case 2026-09-05: the stored password was never applied and
    # Grafana sat on factory admin/admin; review live-fixed via this
    # exact call). The API fails LOUDLY if the default was already
    # changed — the right behavior on a re-run.
    sleep 5
    curl -fsS -u admin:admin -X PUT 127.0.0.1:3000/api/admin/users/1/password \
        -H 'Content-Type: application/json' \
        -d "{\"password\":\"$(sed -n 2p /etc/edda/grafana-admin)\"}" >/dev/null
fi

echo "== verify (loopback) =="
sleep 3
curl -s -o /dev/null -w 'victoria-metrics: %{http_code}\n' 127.0.0.1:8428/health
curl -s -o /dev/null -w 'grafana: %{http_code}\n' 127.0.0.1:3000/api/health
curl -s 127.0.0.1:9100/metrics | grep -c '^node_' | sed 's/^/node-exporter series: /'
curl -s 127.0.0.1:9187/metrics | grep -c '^pg_' | sed 's/^/postgres-exporter series: /'
ss -tlnp | grep -E ':(8428|3000|9100|9187) ' | grep -v 127.0.0.1 && echo "WARNING: something bound beyond loopback" || true
echo "MONITORING-READY (tunnel: ssh -L 3000:127.0.0.1:3000 root@box; creds in /etc/edda/grafana-admin)"
