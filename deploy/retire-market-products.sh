#!/usr/bin/env bash
# One-off, 2026-10-20 (maintainer: "kill it with fire"): retire the
# `community` and `market_daily` products on the box.
#
# No client has fetched either since 0.3.0 made the client journal-only
# (B.4, 2026-09-09). The weekly community build cost 12 min wall and
# 482 CPU-s to stream 104 M rows into a 690 MB file nobody downloaded, two
# versions kept (1.3 GB); market_daily added 59 MB a day (115 MB kept).
#
# What this does, in order, each step idempotent:
#   1. comments the two publish lines out of the deployed timer scripts, so
#      Sunday does not rebuild what the next deploy no longer knows;
#   2. rewrites current.json without the two products, atomically (the
#      service re-reads the file per request; every reader skips unknown
#      keys, so a client holding the old manifest is unaffected);
#   3. deletes the two product directories and the empty legacy `market`.
# A copy of the manifest as it was goes to /var/lib/edda/backups first.
#
# Run as root on the box:  bash deploy/retire-market-products.sh
set -euo pipefail

A=${EDDA_ARTIFACT_DIR:-/var/lib/edda/artifacts}
B=/var/lib/edda/backups

script_of() { systemctl cat "$1" | sed -n 's/^ExecStart=//p' | awk '{print $1}' | head -1; }
retire_line() {  # file, command
    local f=$1 cmd=$2
    if grep -q "^/usr/local/bin/ed-api $cmd\$" "$f"; then
        sed -i "s|^/usr/local/bin/ed-api $cmd\$|# retired 2026-10-20: ed-api $cmd|" "$f"
        echo "retired '$cmd' in $f"
    else
        echo "'$cmd' already absent from $f"
    fi
}
retire_line "$(script_of edda-weekly.service)" publish-community
retire_line "$(script_of edda-daily.service)" publish-market-daily

echo "manifest before: $(python3 -c "import json;print(sorted(json.load(open('$A/current.json'))['products']))")"
mkdir -p "$B"
cp -p "$A/current.json" "$B/current.json.pre-retire-$(date -u +%Y%m%dT%H%M%SZ)"
python3 - "$A/current.json" <<'PY'
import json, os, sys, time
p = sys.argv[1]
m = json.load(open(p))
before = sorted(m["products"])
for k in ("community", "market_daily", "bootstrap"):
    m["products"].pop(k, None)
if sorted(m["products"]) != before:
    m["generated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    tmp = p + ".pending"
    with open(tmp, "w") as f:
        json.dump(m, f, indent=2)
        f.write("\n")
    os.replace(tmp, p)
print("manifest after:", sorted(m["products"]))
PY
chown edda:edda "$A/current.json"
echo "served: $(curl -fsS http://127.0.0.1:8788/v1/manifest | python3 -c 'import sys,json;print(sorted(json.load(sys.stdin)["products"]))')"

for d in community market_daily market; do
    if [ -d "$A/$d" ]; then
        echo "deleting $A/$d ($(du -sh "$A/$d" | cut -f1))"
        rm -rf "$A/$d"
    fi
done
ls "$A"
df -h / | tail -1
