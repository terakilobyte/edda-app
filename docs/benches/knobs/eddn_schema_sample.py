#!/usr/bin/env python3
"""Sample the EDDN firehose for N seconds and count frames per schema and
per journal event (maintainer, 2026-09-09: "if it could have been gotten from
the eddn feed and we just aren't parsing it, we need to fix that
pronto"). The instrument for deciding which schemas/events the writer
must learn. Needs pyzmq.
Usage: python eddn_schema_sample.py [seconds] [--csv out.csv]
"""
import collections
import json
import sys
import time
import zlib

import zmq

secs = 120
csv_out = None
args = sys.argv[1:]
if args and args[0].isdigit():
    secs = int(args[0]); args = args[1:]
if len(args) >= 2 and args[0] == "--csv":
    csv_out = args[1]

ctx = zmq.Context()
s = ctx.socket(zmq.SUB)
s.setsockopt(zmq.SUBSCRIBE, b"")
s.setsockopt(zmq.RCVTIMEO, 5000)
s.connect("tcp://eddn.edcd.io:9500")
schemas = collections.Counter()
events = collections.Counter()
n = 0
t0 = time.time()
while time.time() - t0 < secs:
    try:
        raw = s.recv()
    except zmq.Again:
        continue
    n += 1
    try:
        m = json.loads(zlib.decompress(raw))
    except Exception:
        schemas["undecodable"] += 1
        continue
    ref = m.get("$schemaRef", "?")
    parts = ref.rstrip("/").split("/")
    sch = parts[-2] if len(parts) >= 2 else ref
    schemas[sch] += 1
    if sch == "journal":
        events[m.get("message", {}).get("event", "?")] += 1
took = time.time() - t0
print(f"frames {n} in {took:.0f} s ({n / took:.1f}/s)")
print("schemas:")
for k, v in schemas.most_common():
    print(f"  {k:24} {v:6}  {100 * v / n:5.1f}%")
print("journal events:")
for k, v in events.most_common(30):
    print(f"  {k:24} {v:6}")
if csv_out:
    with open(csv_out, "w", encoding="utf-8") as f:
        f.write(f"# EDDN firehose sample, {took:.0f} s, {n} frames, {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(t0))}\n")
        f.write("kind,name,frames,share_pct\n")
        for k, v in schemas.most_common():
            f.write(f"schema,{k},{v},{100 * v / n:.2f}\n")
        for k, v in events.most_common():
            f.write(f"journal_event,{k},{v},{100 * v / n:.2f}\n")
    print("wrote", csv_out)
