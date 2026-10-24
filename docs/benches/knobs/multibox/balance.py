"""Can Status.json's Balance pick the commander? Per concurrent window: the gap between the two
commanders' credits at their latest LoadGame, against the money each moved inside the window
(sum of |reward| and |cost| fields). Attribution by nearest balance is safe when gap > turnover."""
import sqlite3, sys, json, bisect, datetime as dt, collections
c = sqlite3.connect(sys.argv[1])
label = {}; fid_of_file = {}
for f, raw in c.execute("select file, raw from events where event='Commander'"):
    fid = json.loads(raw).get("FID")
    label.setdefault(fid, chr(ord("A") + len(label))); fid_of_file.setdefault(f, label[fid])
def t(s): return dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").timestamp()
win = {}
for f, a, b in c.execute("select file, min(ts), max(ts) from events group by file"):
    if f in fid_of_file: win[f] = (t(a), t(b), fid_of_file[f])
ivs = sorted(win.values()); overl = []
for i, (a0, a1, la) in enumerate(ivs):
    for b0, b1, lb in ivs[i+1:]:
        if b0 > a1: break
        if la != lb: overl.append((max(a0, b0), min(a1, b1)))
overl.sort()
MONEY = {"MissionCompleted": "Reward", "Bounty": "TotalReward", "MarketSell": "TotalSale", "MarketBuy": "TotalCost",
         "RedeemVoucher": "Amount", "RefuelAll": "Cost", "BuyAmmo": "Cost", "RepairAll": "Cost", "ShipyardBuy": "ShipPrice",
         "ShipyardSell": "ShipPrice", "ModuleBuy": "BuyPrice", "ModuleSell": "SellPrice", "PayFines": "Amount", "PayBounties": "Amount",
         "CarrierDepositFuel": "Total", "CarrierBankTransfer": "Deposit", "SellExplorationData": "TotalEarnings",
         "MultiSellExplorationData": "TotalEarnings", "CarrierFinance": None, "Resurrect": "Cost", "BuyDrones": "TotalCost", "SellDrones": "TotalSale",
         "FactionKillBond": "Reward", "CommunityGoalReward": "Reward", "PowerplaySalary": "Amount", "SquadronCreated": None}
credits = collections.defaultdict(list)  # L -> [(ts, credits)]
money = collections.defaultdict(list)    # L -> [(ts, amount)]
for f, s, ev, raw in c.execute("select file, ts, event, raw from events where event in ('LoadGame', %s) order by ts, file, offset" % ",".join("'%s'" % k for k in MONEY)):
    L = fid_of_file.get(f)
    if not L: continue
    j = json.loads(raw); x = t(s)
    if ev == "LoadGame" and "Credits" in j: credits[L].append((x, j["Credits"]))
    elif MONEY.get(ev) and isinstance(j.get(MONEY[ev]), (int, float)): money[L].append((x, abs(j[MONEY[ev]])))
Ls = sorted(credits)
def last_credits(L, x):
    arr = credits[L]; k = bisect.bisect_right(arr, (x, float("inf"))) - 1
    return arr[k][1] if k >= 0 else None
def turnover(L, s, e):
    arr = money[L]; i = bisect.bisect_left(arr, (s, -1)); j = bisect.bisect_right(arr, (e, float("inf")))
    return sum(v for _, v in arr[i:j])
gaps = []; unsafe = 0; n = 0
for s, e in overl:
    cs = [last_credits(L, s) for L in Ls]
    if None in cs: continue
    n += 1
    gap = abs(cs[0] - cs[1]); to = max(turnover(L, s, e) for L in Ls)
    gaps.append(gap)
    if to >= gap / 2: unsafe += 1
gaps.sort()
print(f"concurrent windows with both balances known {n}; credit gap min {gaps[0]:,} median {gaps[len(gaps)//2]:,}")
print(f"windows where one side's in-window turnover reached half the gap (nearest-balance could flip): {unsafe}")
print("LoadGame credits rows per commander:", {L: len(credits[L]) for L in Ls})
