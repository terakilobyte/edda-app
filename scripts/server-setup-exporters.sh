#!/usr/bin/env bash
# One-shot (idempotent) monitoring-exporter setup for the EDDA API box.
# Run as root on the server:  bash server-setup-exporters.sh
#
# Installs node_exporter (:9100) and postgres_exporter (:9187), both
# loopback-only like everything else VictoriaMetrics scrapes, adds the
# scrape jobs to /etc/edda/vm-scrape.yml, and restarts edda-vm.
# The Postgres role is monitoring-only (pg_monitor); its password is
# generated once into /etc/edda/exporter-pg (0600), same convention as
# the grafana-pg secret next to it.
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get install -y -q prometheus-node-exporter prometheus-postgres-exporter

# --- node exporter: bind loopback only -------------------------------
cat > /etc/default/prometheus-node-exporter <<'EOF'
# EDDA: loopback-only, scraped by the local VictoriaMetrics.
ARGS="--web.listen-address=127.0.0.1:9100"
EOF

# --- postgres exporter: dedicated pg_monitor role --------------------
if [ ! -s /etc/edda/exporter-pg ]; then
    (umask 077; head -c 24 /dev/urandom | base64 | tr -d '/+=' > /etc/edda/exporter-pg)
fi
PW="$(cat /etc/edda/exporter-pg)"
sudo -u postgres psql -v ON_ERROR_STOP=1 -tAc \
    "DO \$\$ BEGIN
        IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'edda_exporter') THEN
            CREATE ROLE edda_exporter LOGIN;
        END IF;
    END \$\$;"
sudo -u postgres psql -v ON_ERROR_STOP=1 -c \
    "ALTER ROLE edda_exporter LOGIN PASSWORD '${PW}'; GRANT pg_monitor TO edda_exporter;"
(umask 077; cat > /etc/default/prometheus-postgres-exporter <<EOF
# EDDA: loopback-only, monitoring-only role (pg_monitor).
DATA_SOURCE_NAME='postgresql://edda_exporter:${PW}@127.0.0.1:5432/edda?sslmode=disable'
ARGS="--web.listen-address=127.0.0.1:9187"
EOF
)

systemctl enable --now prometheus-node-exporter prometheus-postgres-exporter
systemctl restart prometheus-node-exporter prometheus-postgres-exporter

# --- scrape jobs (append once) ---------------------------------------
if ! grep -q "job_name: node" /etc/edda/vm-scrape.yml; then
    cat >> /etc/edda/vm-scrape.yml <<'EOF'
  - job_name: node
    scrape_interval: 15s
    static_configs:
      - targets: ["127.0.0.1:9100"]
        labels:
          service: node
  - job_name: postgres
    scrape_interval: 30s
    static_configs:
      - targets: ["127.0.0.1:9187"]
        labels:
          service: postgres
EOF
fi
systemctl restart edda-vm

# --- verify ----------------------------------------------------------
sleep 20
echo "--- node exporter:"
curl -sf 127.0.0.1:9100/metrics | grep -m1 node_load1 || echo "NODE EXPORTER NOT ANSWERING"
echo "--- postgres exporter (pg_up must be 1):"
curl -sf 127.0.0.1:9187/metrics | grep -E "^pg_up" || echo "POSTGRES EXPORTER NOT ANSWERING"
echo "--- VictoriaMetrics targets:"
curl -sf "http://127.0.0.1:8428/api/v1/query?query=up" | tr ',' '\n' | grep -E "job|value" | head -20
echo "done."
