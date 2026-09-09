#!/usr/bin/env python3
"""Prod API load harness with ROTATING queries (doctrine rules 1, 3, 4).

Why this exists: api-load.sh pins one body per route, and the 2026-09-07
run showed route and trade answering at p50 1 ms server-side — cache
hits after the first request. This harness never repeats a query on the
planner routes: every request draws fresh from a pool of real system
names fetched from /v1/names/complete at startup, so /v1/route and
/v1/trade/search do the work a commander's request does.

PRE-REGISTERED EXPECTATION (2026-09-07, before the first run):
  - route: p50 in the hundreds of ms server-side for bubble pairs, seconds
    for long pairs; it is CPU-bound, so at c=16 on 8 cores the box's CPU
    is the first thing to bend, and at c=64 route p95 more than doubles.
  - trade/search and market/search: Postgres-bound, tens to hundreds of
    ms; with the pool at 30 they should hold to c=32, then queue.
  - names/complete and knowledge/system stay under 20 ms p95 at every c.
  - zero 5xx; 429 = the limiter (open the drop-in for a capacity run).

Usage (stdlib only, no oha):
  docs/benches/knobs/api-load-rotate.py [--api URL] [--duration 30] [--conc "1 4 16 64"]
      [--pool 3000] [--seed 7] > docs/benches/api-load-rotate-YYYY-MM-DD-<who>.csv
Same CSV columns as api-load.sh so the files diff. Latencies include the
runner's RTT; the box's histograms are the shared truth.
"""
import argparse, json, random, string, sys, threading, time, urllib.request, urllib.error
from http.client import HTTPSConnection, HTTPConnection
from urllib.parse import urlparse, quote

COMMODITIES = ["gold", "silver", "palladium", "painite", "tritium", "beryllium", "bertrandite",
               "indite", "gallite", "coltan", "lithium", "food cartridges", "medical diagnostic equipment",
               "insulating membrane", "performance enhancers", "platinum", "osmium", "hydrogen fuel"]


def fetch_json(api, path):
    with urllib.request.urlopen(f"{api}{path}", timeout=30) as r:
        return json.load(r)


def build_pool(api, size, rng):
    """Real system names where commanders actually fly: the sphere around
    Sol (radius 120 ly ≈ 5,000 named systems). The first revision drew
    names from the whole routing index — mostly uninhabited procedural
    systems thousands of ly apart — and every route was an unroutable
    30 s 5xx and every trade search a deep-space scan: a real finding
    (ledgered), but not the load a commander generates."""
    try:
        systems = fetch_json(api, "/v1/knowledge/sphere?x=0&y=0&z=0&radius_ly=120")
        names = sorted({s["name"] for s in systems if s.get("name")})
    except Exception as e:
        print(f"sphere fetch failed: {e}", file=sys.stderr)
        names = []
    rng.shuffle(names)
    return names[:size]


# Long-range destinations for the deliberate far class (the Sol → Colonia
# shape from api-latency.sh), so the long-range planner is measured on
# its own row instead of poisoning the bubble row.
# "Eol Prou RS-T d3-94" was here for the 07:56Z run: it is Colonia's
# pre-rename name and no longer exists, so a quarter of that run's long
# routes were fast 422 unknown_system — the harness's fault, not the
# resolver's (verified: routes to uninhabited index-only systems are 200).
FAR = ["Colonia", "Sagittarius A*", "Jackson's Lighthouse", "Rohini"]


class Worker(threading.Thread):
    def __init__(self, api, make_request, deadline, rng, out):
        super().__init__(daemon=True)
        self.api = urlparse(api); self.make = make_request; self.deadline = deadline
        self.rng = rng; self.out = out

    def run(self):
        conn_cls = HTTPSConnection if self.api.scheme == "https" else HTTPConnection
        conn = conn_cls(self.api.netloc, timeout=60)
        while time.monotonic() < self.deadline:
            method, path, body = self.make(self.rng)
            headers = {"content-type": "application/json"} if body is not None else {}
            t0 = time.perf_counter()
            try:
                conn.request(method, path, body=body, headers=headers)
                resp = conn.getresponse()
                resp.read()
                self.out.append((time.perf_counter() - t0, resp.status))
            except Exception:
                self.out.append((time.perf_counter() - t0, 0))
                try:
                    conn.close()
                except Exception:
                    pass
                conn = conn_cls(self.api.netloc, timeout=60)


def pct(xs, p):
    if not xs:
        return ""
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(p * len(xs)))] * 1000, 1)


def rung(name, api, conc, duration, make_request, seed):
    out = []
    deadline = time.monotonic() + duration
    workers = [Worker(api, make_request, deadline, random.Random(seed * 1000 + i), out) for i in range(conc)]
    t0 = time.monotonic()
    for w in workers:
        w.start()
    for w in workers:
        w.join(duration + 65)
    elapsed = time.monotonic() - t0
    lat = [t for t, s in out if s]
    codes = {}
    for _, s in out:
        codes[s] = codes.get(s, 0) + 1
    ok = sum(v for k, v in codes.items() if 200 <= k < 300)
    r429 = codes.get(429, 0)
    r5 = sum(v for k, v in codes.items() if 500 <= k < 600)
    transport = codes.get(0, 0)
    other = len(out) - ok - r429 - r5 - transport
    # The full code distribution, so an "other" bucket is never opaque
    # (2026-09-07: route_long at c=1 showed 2 of 8 "other" and nobody
    # could tell a fast 4xx "no route" from a broken answer).
    dist = ";".join(f"{k}:{v}" for k, v in sorted(codes.items()))
    print(f"{name},{conc},{len(out)},{round(len(out)/elapsed,2)},{pct(lat,0.5)},{pct(lat,0.95)},{pct(lat,0.99)},"
          f"{ok},{r429},{r5},{other},{transport},{dist}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--api", default="https://api.edda-app.com")
    ap.add_argument("--duration", type=int, default=30)
    ap.add_argument("--conc", default="1 4 16 64")
    ap.add_argument("--pool", type=int, default=3000)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--only", default="", help="comma list of rung names to run (default all), e.g. route_bubble_rotating,route_long_rotating")
    a = ap.parse_args()
    only = {s.strip() for s in a.only.split(",") if s.strip()}
    rng = random.Random(a.seed)
    print(f"# api-load-rotate {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())} api={a.api} pool building…", file=sys.stderr)
    pool = build_pool(a.api, a.pool, rng)
    if len(pool) < 100:
        print(f"only {len(pool)} names in the pool; is /v1/names/complete deployed?", file=sys.stderr)
        sys.exit(1)
    print(f"# pool: {len(pool)} system names", file=sys.stderr)
    j = json.dumps

    def route(r):
        return ("POST", "/v1/route", j({"from": r.choice(pool), "to": r.choice(pool), "range_ly": r.choice([30, 40, 50, 65])}))

    def route_long(r):
        return ("POST", "/v1/route", j({"from": r.choice(pool), "to": r.choice(FAR), "range_ly": r.choice([50, 65])}))

    def trade(r):
        return ("POST", "/v1/trade/search", j({"system": r.choice(pool), "radius_ly": r.choice([30, 40, 60]), "limit": 10}))

    def market(r):
        return ("POST", "/v1/market/search", j({"kind": "commodity", "text": r.choice(COMMODITIES), "system": r.choice(pool),
                                                 "radius_ly": r.choice([30, 50, 80]), "side": r.choice(["sell", "buy"]), "limit": 10}))

    # The API-only client's trade request: a ship block makes the server
    # build the whole ProfitReport (round trips) instead of the legacy legs.
    def trade_report(r):
        return ("POST", "/v1/trade/search", j({"system": r.choice(pool),
                                                "ship": {"cargo_capacity": r.choice([256, 512, 720]), "jump_range_ly": r.choice([25, 30, 40]), "laden_range_ly": r.choice([18, 22, 30])},
                                                "constraints": {"radius_ly": r.choice([40, 60]), "max_age_hours": 48}}))

    SERVICES = ["market", "outfitting", "shipyard", "interstellar_factors", "technology_broker",
                "universal_cartographics", "material_trader", "refuel", "repair", "restock", "missions"]

    def stations_near(r):
        return ("GET", f"/v1/stations?near={quote(r.choice(pool))}&service={r.choice(SERVICES)}&radius_ly={r.choice([30, 50, 80])}&limit=10", None)

    def complete(r):
        p = r.choice(string.ascii_lowercase) + r.choice(string.ascii_lowercase) + r.choice(["", r.choice(string.ascii_lowercase)])
        return ("GET", f"/v1/names/complete?kind=system&prefix={p}&limit=12", None)

    def lookup(r):
        return ("GET", f"/v1/knowledge/system?name={quote(r.choice(pool))}", None)

    print("route,concurrency,requests,rps,p50_ms,p95_ms,p99_ms,http_2xx,http_429,http_5xx,http_other,transport_errors,codes")
    rungs = [
        ("names_complete_rotating", complete),
        ("knowledge_system_rotating", lookup),
        ("market_search_rotating", market),
        ("trade_search_rotating", trade),
        ("trade_report_rotating", trade_report),
        ("stations_near_rotating", stations_near),
        ("route_bubble_rotating", route),
        ("route_long_rotating", route_long),
    ]
    for c in [int(x) for x in a.conc.split()]:
        for name, make in rungs:
            if only and name not in only:
                continue
            rung(name, a.api, c, a.duration, make, a.seed)


if __name__ == "__main__":
    main()
