#!/usr/bin/env python3
"""Does straight-line ordering of an engineer tour cost jumps? The tour
orders stops on straight-line distance (exact for <= 8 stops); the legs
are then plotted by the route server. This knob plots the legs of the
straight-line optimum and of two alternatives (nearest-neighbour, and the
optimum reversed) against a server and sums the jumps, so the ordering
choice is measured, not assumed (ledger, 2026-10-14).

    python docs/benches/knobs/engineer_tour_order.py [api] [start] [range_ly]

Coordinates come from EDSM's system API (the same source commanders use
by hand); the engineers are the vendored ship engineers in the bubble.
"""
import itertools
import json
import math
import sys
import time
import urllib.parse
import urllib.request

API = sys.argv[1] if len(sys.argv) > 1 else "https://api.edda-app.com"
START = sys.argv[2] if len(sys.argv) > 2 else "Sol"
RANGE = float(sys.argv[3]) if len(sys.argv) > 3 else 72.0
# The bubble cluster the ledger named plus the rest of the core engineers
# a build plan commonly sends a commander to (7 stops: exact enumeration).
ENGINEERS = [
    ("Felicity Farseer", "Deciat"), ("Elvira Martuuk", "Khun"), ("Selene Jean", "Kuk"),
    ("Tod McQuinn", "Wolf 397"), ("Didi Vatermann", "Leesti"), ("Lei Cheung", "Laksak"), ("The Dweller", "Wyrd"),
]


def edsm_pos(name):
    q = urllib.parse.urlencode({"systemName": name, "showCoordinates": 1})
    req = urllib.request.Request("https://www.edsm.net/api-v1/system?" + q, headers={"User-Agent": "EDDA-bench/engineer_tour_order"})
    c = json.load(urllib.request.urlopen(req, timeout=30))["coords"]
    return (c["x"], c["y"], c["z"])


def dist(a, b):
    return math.dist(a, b)


def path_len(start, pts, order):
    here, total = start, 0.0
    for i in order:
        total += dist(here, pts[i])
        here = pts[i]
    return total


def exact(start, pts):
    return min(itertools.permutations(range(len(pts))), key=lambda o: path_len(start, pts, o))


def nearest(start, pts):
    left, here, out = list(range(len(pts))), start, []
    while left:
        i = min(left, key=lambda k: dist(here, pts[k]))
        left.remove(i)
        out.append(i)
        here = pts[i]
    return out


def jumps(frm, to):
    body = json.dumps({"from": frm, "to": to, "range_ly": RANGE, "supercharge": True, "thorough": False}).encode()
    req = urllib.request.Request(API + "/v1/route", data=body, headers={"Content-Type": "application/json", "User-Agent": "EDDA-bench/engineer_tour_order"})
    try:
        d = json.load(urllib.request.urlopen(req, timeout=200))
        return d.get("jumps")
    except urllib.error.HTTPError as e:
        return f"HTTP {e.code}"


def main():
    names = [n for n, _ in ENGINEERS]
    systems = [s for _, s in ENGINEERS]
    pts = [edsm_pos(s) for s in systems]
    start = edsm_pos(START)
    orders = {"straight-line exact": list(exact(start, pts)), "nearest-neighbour": nearest(start, pts)}
    orders["exact reversed"] = list(reversed(orders["straight-line exact"]))
    print("order,stops,straight_ly,total_jumps,legs")
    for label, o in orders.items():
        here, total_j, legs = START, 0, []
        for i in o:
            j = jumps(here, systems[i])
            legs.append(f"{here}->{systems[i]}={j}")
            total_j += j if isinstance(j, int) else 0
            here = systems[i]
            time.sleep(0.3)
        print(f"{label},{' > '.join(names[i] for i in o)},{path_len(start, pts, o):.0f},{total_j},\"{' | '.join(legs)}\"")


if __name__ == "__main__":
    main()
