"""Share of EDDN outfitting messages on the v3 schema (per-module id /
BuyPrice / BuyMercCoinsPrice) versus v2 (bare names), and how many v3
boards carry merc-coin-only modules. 2026-09-29: the market tab listed
pre-engineered merc-coin variants as credit purchases because our
decoder flattened v3 entries to names.

    python3 eddn_outfitting_v3_share.py [seconds=180]

Prints counts and a few example merc-only symbols. Needs pyzmq.
"""
import json, sys, time, zlib, collections
import zmq

seconds = int(sys.argv[1]) if len(sys.argv) > 1 else 180
ctx = zmq.Context(); s = ctx.socket(zmq.SUB); s.setsockopt(zmq.SUBSCRIBE, b""); s.setsockopt(zmq.RCVTIMEO, 5000)
s.connect("tcp://eddn.edcd.io:9500")
c = collections.Counter(); merc_syms = collections.Counter(); uploaders = collections.Counter()
t0 = time.time()
while time.time() - t0 < seconds:
    try: raw = s.recv()
    except zmq.Again: continue
    try: m = json.loads(zlib.decompress(raw))
    except Exception: c["undecodable"] += 1; continue
    ref = m.get("$schemaRef", "")
    if "/outfitting/" not in ref: c["other"] += 1; continue
    ver = ref.rsplit("/", 1)[-1].split("/")[0]
    mods = m.get("message", {}).get("modules", [])
    shape = "objects" if mods and isinstance(mods[0], dict) else "strings"
    c[f"outfitting v{ver} {shape}"] += 1
    uploaders[(ver, m.get("header", {}).get("softwareName", "?"))] += 1
    if shape == "objects":
        priced = [x for x in mods if isinstance(x, dict) and "BuyPrice" in x]
        merc_only = [x for x in priced if x.get("BuyPrice", 0) == 0 and x.get("BuyMercCoinsPrice", 0) > 0]
        c["v3 boards with prices"] += bool(priced)
        c["v3 boards with merc-only modules"] += bool(merc_only)
        for x in merc_only: merc_syms[x.get("Name", "").lower()] += 1
print(f"sampled {seconds} s")
for k, v in sorted(c.items()): print(f"  {k}: {v}")
print("uploaders by version:")
for (ver, sw), n in uploaders.most_common(12): print(f"  v{ver} {sw}: {n}")
print("merc-only symbols seen (top 10):", merc_syms.most_common(10))
