#!/usr/bin/env bash
# Trade-gate sweep (doctrine rules 1, 3, 4). Sibling of plot-gate-sweep.sh.
#
# TradeService ran ONE search at a time (Semaphore(1), 15 s wait then
# 429), sized when the query cost ~3 s. After the fresh-market index and
# the cell predicate (2026-09-07) a report costs ~160–260 ms server-side,
# and the flight-binary ladder showed both trade forms queueing behind
# the single runner: trade_report p50 484 ms at c=4 → 1.76 s / p95 26 s
# at c=16. For each EDDA_API_TRADE_CONCURRENCY value: drop-in, restart,
# readyz, then the SAME instruments — the rotating harness's two trade
# rungs (legacy trade_search and the `ship` trade_report) at c=4 and
# c=16, and trade-behind-plots as the CPU-sharing control. Limiter is
# opened for the window and restored by the caller; the gate drop-in is
# removed at the end.
#
# PRE-REGISTERED (2026-09-08, before the first run), pool cores−2 niced,
# plot gate 4+2 lanes, pool 30:
#   gate 1 → the flight-ladder numbers (control): report 233 req / p50
#            ~480 ms at c=4; 87 req / p50 ~1.8 s / p95 ~26 s at c=16.
#   gate 2 → served reports at c=16 roughly ×2, p95 halves; c=4 p50
#            within ±20 %; trade-behind-plots within ±30 %.
#   gate 3 → served ×3 or the first sign of Postgres/CPU binding (pool
#            idle → 0 or report build ms rising); trade-behind-plots
#            degrades if three concurrent reports contend with two plots
#            for the six planner threads' leftovers — that is the number
#            that says where the gate belongs.
#
#   SSH_BOX=edda docs/benches/knobs/trade-gate-sweep.sh "1 2 3" [duration_s=30]
#     > docs/benches/trade-gate-sweep-YYYY-MM-DD.csv
set -uo pipefail
VALUES="${1:-1 2 3}"; DUR="${2:-30}"
SSH_BOX="${SSH_BOX:-edda}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DROPIN=/etc/systemd/system/edda-api.service.d/tradegate.conf

set_gate() {
  ssh -o ConnectTimeout=20 "$SSH_BOX" "set -e
    if [ -n '$1' ]; then printf '[Service]\nEnvironment=EDDA_API_TRADE_CONCURRENCY=%s\n' '$1' > $DROPIN; else rm -f $DROPIN; fi
    systemctl daemon-reload; systemctl restart edda-api.service
    for i in \$(seq 1 90); do curl -sf http://127.0.0.1:8787/readyz >/dev/null && break; sleep 1; done
    journalctl -u edda-api.service --since '2 min ago' --no-pager | grep -o 'trade gate sized.*' | tail -1 | cut -c1-60
    sudo -u postgres psql -d edda -Atc 'SELECT pg_stat_statements_reset()' >/dev/null"
}
box_sample() {
  ssh -o ConnectTimeout=20 "$SSH_BOX" '
    q() { curl -s "http://127.0.0.1:8428/api/v1/query" --data-urlencode "query=$1" | python3 -c "import sys,json
r=json.load(sys.stdin)[\"data\"][\"result\"]
print(round(float(r[0][\"value\"][1]),2) if r else \"\")"; }
    printf "box,load1=%s,pool_idle=%s,pool_conns=%s\n" "$(cut -d" " -f1 /proc/loadavg)" "$(q edda_db_pool_idle)" "$(q edda_db_pool_connections)"' 2>/dev/null
}
trade_stats() {
  ssh -o ConnectTimeout=20 "$SSH_BOX" "sudo -u postgres psql -d edda -At -F',' -c \"SELECT calls, round(mean_exec_time::numeric), round(max_exec_time::numeric) FROM pg_stat_statements WHERE dbid=(SELECT oid FROM pg_database WHERE datname='edda') AND query ILIKE '%best_buy%' ORDER BY calls DESC LIMIT 1\""
}

echo "gate,instrument,requests,rps,p50_ms,p95_ms,p99_ms,http_2xx,http_429,http_5xx,http_other,transport_errors,codes"
for g in $VALUES; do
  echo "# gate $g: $(set_gate "$g")"
  box_sample | sed "s/^/# gate $g /"
  python3 "$HERE/api-load-rotate.py" --duration "$DUR" --conc "4 16" --only trade_search_rotating,trade_report_rotating 2>/dev/null \
    | grep -v '^route,' | sed "s/^/$g,/"
  echo "$g,trade_pg_stats,$(trade_stats),,,,,,,,,"
  python3 "$HERE/trade-behind-plots.py" 50 2>/dev/null | sed "s/^/# gate $g /"
  box_sample | sed "s/^/# gate $g /"
done
echo "# restoring code default: $(set_gate "")"
