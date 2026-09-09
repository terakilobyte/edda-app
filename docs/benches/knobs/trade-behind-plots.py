# Trade-behind-plots bench: 2 threads plot fresh long routes continuously for DUR s;
# after a 5 s warm-up the main thread runs 5 cold trade searches through the API.
# Prints wire times; pg_stat_statements is reset before and read after by the caller.
import json, random, sys, threading, time, urllib.request
API="https://api.edda-app.com"; DUR=int(sys.argv[1]) if len(sys.argv)>1 else 50
rng=random.Random(11)
pool=[s["name"] for s in json.load(urllib.request.urlopen(f"{API}/v1/knowledge/sphere?x=0&y=0&z=0&radius_ly=120", timeout=30))]
FAR=["Colonia","Sagittarius A*","Jackson's Lighthouse","Rohini"]
stop=time.monotonic()+DUR; plots=[]
def post(path, body, timeout=120):
    req=urllib.request.Request(f"{API}{path}", data=json.dumps(body).encode(), headers={"content-type":"application/json"})
    t0=time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r: r.read(); code=r.status
    except urllib.error.HTTPError as e: code=e.code
    except Exception: code=0
    return code, time.perf_counter()-t0
def plotter(seed):
    # Fixed seeds: the destination draw decides the plot times (Colonia
    # ~2 s, Sag A*/Rohini 20–26 s), so an unseeded run is not comparable
    # to the last one (2026-09-07: 12 vs 6 plots served read as a
    # regression and was the draw).
    r=random.Random(seed)
    while time.monotonic()<stop:
        code,dt=post("/v1/route", {"from":r.choice(pool),"to":r.choice(FAR),"range_ly":r.choice([50,65])})
        plots.append((code,dt))
ths=[threading.Thread(target=plotter,args=(101+i,),daemon=True) for i in range(2)]
[t.start() for t in ths]
time.sleep(5)
for s in ["Wyrd","Eravate","LP 98-132","Kremainn","Shinrarta Dezhra"]:
    code,dt=post("/v1/trade/search", {"system":s,"radius_ly":40,"limit":10})
    print(f"trade {s:18s} {code} {dt:6.2f} s (behind 2 plots)", flush=True)
[t.join() for t in ths]
ok=[dt for c,dt in plots if c==200]; other={}
for c,_ in plots: other[c]=other.get(c,0)+1
print(f"plots: {len(plots)} codes={other} served p50={sorted(ok)[len(ok)//2]:.2f}s max={max(ok):.2f}s" if ok else f"plots: {len(plots)} codes={other}")
