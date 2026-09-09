#!/usr/bin/env bash
# Plot-gate sweep (API-only spec, Phase A step 2; doctrine rules 1, 3, 4).
#
# For each EDDA_API_PLOT_CONCURRENCY value: write the drop-in on the box,
# restart ed-api, wait for /readyz, then measure with the SAME two
# instruments: the rotating harness's route rungs at c=16 from this
# machine (served plots, 503 %, p50 — read with the codes column) and
# trade-behind-plots (five cold trade searches through the API while two
# threads plot fresh long routes). The drop-in is removed at the end so
# the code default is what runs.
#
# PRE-REGISTERED (2026-09-07, before the first run), pool = cores − 2 = 6
# niced threads (Phase A step 1):
#   gate 2 → the step-1 numbers (control).
#   gate 4 → served plots within ±20 % of gate 2; 503s at c=16 from
#            ~75 % toward ~0; plot p50 ~245 ms → ~700 ms (more in flight,
#            each slower); trade-behind-plots within ±30 % of gate 2.
#   gate 6 → served plots FALL below gate 4 (six fan-outs on six
#            threads) and trade-behind-plots degrades — that is the
#            number that says where the gate belongs.
#
#   SSH_BOX=edda docs/benches/knobs/plot-gate-sweep.sh "2 4 6" [duration_s=30]
#     > docs/benches/plot-gate-sweep-YYYY-MM-DD.csv
set -uo pipefail
VALUES="${1:-2 4 6}"; DUR="${2:-30}"
SSH_BOX="${SSH_BOX:-edda}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DROPIN=/etc/systemd/system/edda-api.service.d/plotgate.conf

set_gate() {
  ssh -o ConnectTimeout=20 "$SSH_BOX" "set -e
    if [ -n '$1' ]; then printf '[Service]\nEnvironment=EDDA_API_PLOT_CONCURRENCY=%s\n' '$1' > $DROPIN; else rm -f $DROPIN; fi
    systemctl daemon-reload; systemctl restart edda-api.service
    for i in \$(seq 1 90); do curl -sf http://127.0.0.1:8787/readyz >/dev/null && break; sleep 1; done
    journalctl -u edda-api.service --since '2 min ago' --no-pager | grep -o 'plot gate sized.*' | tail -1 | cut -c1-80
    sudo -u postgres psql -d edda -Atc 'SELECT pg_stat_statements_reset()' >/dev/null"
}
trade_stats() {
  ssh -o ConnectTimeout=20 "$SSH_BOX" "sudo -u postgres psql -d edda -At -F',' -c \"SELECT calls, round(mean_exec_time::numeric), round(max_exec_time::numeric) FROM pg_stat_statements WHERE dbid=(SELECT oid FROM pg_database WHERE datname='edda') AND query ILIKE '%best_buy%'\""
}

echo "gate,instrument,requests,rps,p50_ms,p95_ms,p99_ms,http_2xx,http_429,http_5xx,http_other,transport_errors,codes"
for g in $VALUES; do
  echo "# gate $g: $(set_gate "$g")"
  python3 "$HERE/api-load-rotate.py" --duration "$DUR" --conc 16 --only route_bubble_rotating,route_long_rotating 2>/dev/null \
    | grep -v '^route,' | sed "s/^/$g,/"
  # trade-behind-plots: wire times, then pg mean/max for the trade statement during it
  out="$(python3 "$HERE/../../../docs/benches/knobs/trade-behind-plots.py" 50 2>/dev/null || python3 "$HERE/trade-behind-plots.py" 50 2>/dev/null)"
  echo "$out" | sed "s/^/# gate $g /"
  echo "$g,trade_behind_plots_pg,$(trade_stats),,,,,,,,,"
done
echo "# restoring code default: $(set_gate "")"
