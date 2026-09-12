# How much does "discounted only" leave? Cross-reference live market rows
# against the live Powerplay map. Measurement for the 2026-09-12 feature.
import json, urllib.request, collections, sys
sys.stdout.reconfigure(encoding='utf-8')
API = "https://api.edda-app.com"
def post(path, body):
    r = urllib.request.Request(API + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=180))
def get(path):
    return json.load(urllib.request.urlopen(API + path, timeout=180))

RAD = 150
sphere = get(f"/v1/knowledge/sphere?x=0&y=0&z=0&radius={RAD}")
sysx = sphere if isinstance(sphere, list) else sphere.get("systems") or sphere.get("results") or []
power = {s["name"]: (s.get("controllingPower"), s.get("powerState")) for s in sysx}
CONTROLLED = {"Stronghold", "Fortified"}
HELD = CONTROLLED | {"Exploited"}

def discount(kind, symbol, station, system):
    p, st = power.get(system, (None, None))
    best, why = 0.0, None
    if p == "Li Yong-Rui" and st in HELD: best, why = 15.0, "Li Yong-Rui space"
    if kind == "module" and symbol.startswith("hpt_") and p == "Jerome Archer":
        if st in CONTROLLED and 20.0 > best: best, why = 20.0, "Archer control space"
        elif st == "Exploited" and 10.0 > best: best, why = 10.0, "Archer exploited space"
    if kind == "module" and any(x in symbol for x in ("cargorack", "hullreinforcement")) and p == "Edmund Mahon" and st in CONTROLLED and 20.0 > best:
        best, why = 20.0, "Mahon control space"
    if kind == "ship" and symbol in ("empire_eagle","empire_courier","empire_trader","cutter") and p == "Denton Patreus" and st in CONTROLLED and 10.0 > best:
        best, why = 10.0, "Patreus control space"
    if station == "Jameson Memorial" and system == "Shinrarta Dezhra" and 10.0 > best: best, why = 10.0, "Jameson Memorial"
    return best, why

for kind, text in (("ship", "anaconda"), ("ship", "cutter"), ("module", "hpt_beamlaser"), ("module", "int_cargorack")):
    rows = post("/v1/market/search", {"kind": kind, "text": text, "system": "Sol", "radius_ly": RAD, "limit": 75}).get("results", [])
    hits = [(r, *discount(kind, r.get("symbol",""), r.get("station",""), r.get("system",""))) for r in rows]
    disc = [h for h in hits if h[1] > 0]
    by = collections.Counter(h[2] for h in disc)
    print(f"{kind:6} {text:16} {len(rows):3} rows -> {len(disc):3} discounted  {dict(by)}")
    for r, pct, why in disc[:2]:
        print(f"         e.g. {r['station']} / {r['system']}  −{pct}%  ({why})")
