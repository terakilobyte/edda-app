#!/usr/bin/env python
"""rejudge_seconds.py <arm.jsonl> <route_dir> <tag>

Add `seconds_v2` to every row of a matrix arm from its route files
(`<route_dir>/<tag>-<slug>.route.jsonl`, written by edda_matrix.sh): the
R2 judge, plain 53 s, boosted 73 s, passing scoop 6 s, full stop 120 s plus
tonnes over 1.245 t/s. Rows without a route keep no figure. The renderer's
`--judge seconds` then compares this field, so arms planned under
different judges are read in one currency.
"""
import json
import os
import re
import sys

T_PLAIN, T_BOOST, T_PASS, T_STOP, RATE = 53.0, 73.0, 6.0, 120.0, 1.245


def slug(f, t):
    return re.sub(r"[^A-Za-z0-9_.-]", "_", f"{f}__{t}")


def seconds(route):
    hops = route.get("hops") or []
    plain = boosted = passing = stops = 0
    scooped = 0.0
    prev = None
    for h in hops[1:] if hops else []:
        if h.get("boosted"):
            boosted += 1
        else:
            plain += 1
    for h in hops:
        f = h.get("fuel_after")
        if f is not None and prev is not None and f > prev + 0.05:
            scooped += f - prev
            if h.get("refuel"):
                stops += 1
            else:
                passing += 1
        if f is not None:
            prev = f
    return plain * T_PLAIN + boosted * T_BOOST + passing * T_PASS + stops * T_STOP + scooped / RATE


def main(argv):
    arm, route_dir, tag = argv[1], argv[2], argv[3]
    rows = [json.loads(l) for l in open(arm, encoding="utf-8") if l.strip()]
    done = 0
    for r in rows:
        path = os.path.join(route_dir, f"{tag}-{slug(r['from'], r['to'])}.route.jsonl")
        if r.get("jumps") is None or not os.path.exists(path):
            continue
        last = None
        for line in open(path, encoding="utf-8"):
            if line.strip():
                last = json.loads(line)
        if last:
            r["seconds_v2"] = round(seconds(last))
            done += 1
    with open(arm, "w", encoding="utf-8", newline="\n") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    print(f"{done} of {len(rows)} rows re-judged into {arm}")


if __name__ == "__main__":
    main(sys.argv)
