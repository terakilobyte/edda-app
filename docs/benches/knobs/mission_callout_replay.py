"""Replay the boss's journal kill by kill and print what watcher::mission_progress
would have said under the old list order (newest accepted first) and the new
HUD order (active, expiry, kills remaining), plus how inferred completion
compares with the game's own MissionRedirected."""
import sqlite3, json, sys, collections
sys.stdout.reconfigure(encoding='utf-8')
SINCE = sys.argv[1] if len(sys.argv) > 1 else '2026-09-09'
c = sqlite3.connect(r"file:C:/Users/terak/AppData/Local/edda/edda.sqlite3?mode=ro", uri=True)
rows = c.execute("""SELECT ts, event, raw FROM events
    WHERE event IN ('MissionAccepted','MissionCompleted','MissionRedirected','MissionFailed','MissionAbandoned','Bounty','FactionKillBond')
      AND ts >= ? ORDER BY ts, file, offset""", (SINCE,)).fetchall()

M = collections.OrderedDict()   # acceptance order
redirected = {}                 # id -> ts of the game's own completion signal
inferred = {}                   # id -> ts of the kill that crossed the threshold
def rank_new(m): return (m['status'] != 'Active', m['expiry'] or '~', (m['need'] - m['done']) if m['need'] else 1 << 60, m['accepted'], m['id'])
kills = 0; silent_old = silent_new = 0; multi = collections.Counter(); said_old = collections.Counter(); said_new = collections.Counter()
log = []
for ts, e, raw in rows:
    v = json.loads(raw)
    if e == 'MissionAccepted':
        M[v['MissionID']] = dict(id=v['MissionID'], accepted=ts, giver=v.get('Faction', ''), need=v.get('KillCount'), done=0,
                                 status='Active', expiry=v.get('Expiry'), target=v.get('TargetFaction'), station=v.get('DestinationStation'))
    elif e in ('MissionCompleted', 'MissionAbandoned', 'MissionFailed'):
        if v.get('MissionID') in M: M[v['MissionID']]['status'] = e[7:]
    elif e == 'MissionRedirected':
        m = M.get(v.get('MissionID'))
        if m:
            redirected[m['id']] = ts
            if m['status'] == 'Active':
                m['status'] = 'ReadyToTurnIn'; m['done'] = m['need'] or m['done']
    else:
        victim = v.get('VictimFaction')
        if not any(m['status'] == 'Active' and m['need'] and m['target'] == victim for m in M.values()):
            continue
        kills += 1
        crossed = []
        for m in M.values():
            if m['status'] == 'Active' and m['need'] and m['target'] == victim:
                m['done'] += 1
                if m['done'] >= m['need']:
                    m['done'] = m['need']; m['status'] = 'ReadyToTurnIn'; crossed.append(m); inferred[m['id']] = ts
        multi[len(crossed)] += 1
        live = [m for m in M.values() if m['status'] in ('Active', 'ReadyToTurnIn')]
        def callout(order):
            for m in order:
                if m['need'] and m['target'] == victim:
                    return m, ('complete' if m['done'] >= m['need'] else 'progress')
            return None, None
        old = callout(list(reversed(live)))          # missions() reverses: newest accepted first
        new = callout(sorted(live, key=rank_new))
        said_old[old[1]] += 1; said_new[new[1]] += 1
        if crossed and old[0] not in crossed: silent_old += 1
        if crossed and new[0] not in crossed: silent_new += 1
        if crossed or kills % 25 == 0:
            fmt = lambda o: f"{o[1]:8} {o[0]['done']}/{o[0]['need']} {o[0]['giver'][:22]}" if o[0] else '-'
            log.append(f"{ts[5:16]} kill#{kills:<4} crossed={[m['giver'][:18] for m in crossed]}\n      old: {fmt(old)}\n      new: {fmt(new)}")
print('\n'.join(log))
print(f"\nkills counted toward a mission: {kills}")
print(f"kills that completed N missions at once: {dict(sorted(multi.items()))}")
print(f"callout kinds  old={dict(said_old)}  new={dict(said_new)}")
print(f"completions passed without a 'Mission complete' callout: old order {silent_old}, new order {silent_new} (of {sum(k*n for k,n in multi.items() if k)} completions on {sum(n for k,n in multi.items() if k)} kills)")
print("\ninferred completion vs the game's MissionRedirected:")
early = late = exact = 0
for mid, m in M.items():
    if mid in inferred or mid in redirected:
        a, b = inferred.get(mid), redirected.get(mid)
        tag = '  (no redirect seen)' if not b else ('  (never inferred)' if not a else '')
        if a and b:
            if a < b: early += 1
            elif a > b: late += 1
            else: exact += 1
        print(f"  {m['giver'][:26]:26} {m['need']:>3}  inferred {a and a[5:19]}  redirected {b and b[5:19]}{tag}")
print(f"inferred earlier than the game: {early}, later: {late}, same second: {exact}")
