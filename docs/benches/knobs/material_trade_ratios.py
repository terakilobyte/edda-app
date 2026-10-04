"""Replay every MaterialTrade in the commander's journal against EDDA's
trade-ratio model (crates/ed-journal/src/mat_trade.rs) and write the
verdict CSV. Measurement before building (2026-10-04).

Model: same category, d grades down -> 1 : 3^d; u grades up -> 6^u : 1.
Across categories (same trader type): same grade 6 : 1; d down -> 2 : 3^(d-1);
u up -> 6^(u+1) : 1. Guardian/Thargoid rows (category None) do not trade.

    python docs/benches/knobs/material_trade_ratios.py > docs/benches/2026-10-04-material-trade-ratios.csv
"""
import csv, glob, json, os, sys
from collections import Counter

J = os.path.expanduser(r"~/Saved Games/Frontier Developments/Elite Dangerous")
table = {}
with open("crates/ed-journal/data/material.csv", encoding="utf-8") as f:
    for r in csv.DictReader(f):
        table[r["symbol"].lower()] = (r["type"], r["category"], int(r["rarity"]), r["name"])

def ratio(src, dst):
    (tk, ck, gk, _), (td, cd, gd, _) = table[src], table[dst]
    if tk != td or ck == "None" or cd == "None":
        return None
    d = gk - gd
    if ck == cd:
        if d == 0: return None
        return (1, 3 ** d) if d > 0 else (6 ** -d, 1)
    if d == 0: return (6, 1)
    return (2, 3 ** (d - 1)) if d > 0 else (6 ** (-d + 1), 1)

rows, verdict = [], Counter()
for p in sorted(glob.glob(os.path.join(J, "Journal.*.log"))):
    for line in open(p, encoding="utf-8", errors="replace"):
        if '"MaterialTrade"' not in line: continue
        v = json.loads(line)
        s, d = v["Paid"]["Material"].lower(), v["Received"]["Material"].lower()
        pq, rq = v["Paid"]["Quantity"], v["Received"]["Quantity"]
        r = ratio(s, d)
        if r is None:
            ok = "no-model"
        else:
            g, rc = r
            ok = "ok" if pq % g == 0 and rq == pq // g * rc else "MISMATCH"
        verdict[ok] += 1
        rows.append((v["timestamp"], v["TraderType"], s, table[s][2], table[s][1], pq, d, table[d][2], table[d][1], rq, f"{r[0]}:{r[1]}" if r else "", ok))

w = csv.writer(sys.stdout, lineterminator="\n")
w.writerow([f"# material trade ratios vs the commander's journal: {dict(verdict)} -- verdict: {'MODEL HOLDS' if verdict['MISMATCH'] == 0 and verdict['no-model'] == 0 else 'CHECK'}"])
w.writerow(["ts", "trader", "paid", "paid_grade", "paid_cat", "paid_qty", "received", "recv_grade", "recv_cat", "recv_qty", "model_ratio", "verdict"])
w.writerows(rows)
