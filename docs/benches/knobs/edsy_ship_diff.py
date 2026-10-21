"""Diff two EDSY databases ship by ship (every attribute and slot list).

EDSY (edsy.org) is the reference the slot tables are compiled from; when
a game patch lands, its one commit that "updated many ship attributes"
is the cleanest before/after of the rebalance there is. Convert both
revisions with eddb_to_json.js and diff them here; the output is the
patch as EDSY read it, one ship per block, to pin against the dev log and
the journal (the journal wins, then EDCD, then EDSY: ground-every-fact).

    node docs/benches/knobs/eddb_to_json.js <eddb.before.js> before.json
    node docs/benches/knobs/eddb_to_json.js <eddb.after.js>  after.json
    python docs/benches/knobs/edsy_ship_diff.py before.json after.json

First use: docs/benches/2026-10-20-anaconda-mkii-edcd-crossref.csv (game
patch 4.4.2.0, EDSY commits 42dfa51e -> a0942192).
"""
import json
import sys

SKIP = {"module", "stock"}  # per-hull armour prices and the stock fit: noise for a rebalance


def main(before, after):
    old = json.load(open(before, encoding="utf-8"))["ship"]
    new = json.load(open(after, encoding="utf-8"))["ship"]
    for sid in sorted(set(old) | set(new), key=int):
        o, n = old.get(sid), new.get(sid)
        if o is None:
            print(f"NEW SHIP {sid} {n['name']} ({n['fdname']})")
            continue
        if n is None:
            print(f"GONE {sid} {o['name']} ({o['fdname']})")
            continue
        diffs = [f"{k}: {o.get(k)} -> {n.get(k)}" for k in sorted(set(o) | set(n)) if k not in SKIP and o.get(k) != n.get(k)]
        if diffs:
            print(f"{n['name']} ({n['fdname']}):")
            for d in diffs:
                print("   ", d)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
