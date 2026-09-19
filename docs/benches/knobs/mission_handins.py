"""Read-only over the maintainer's store: for every massacre mission, the hand-in at acceptance,
where he was docked when he accepted it, and where the game's redirect sent him; plus the
stations he actually docked at during the mission's life. CSV to stdout."""
import sqlite3, json, sys, csv, io, collections
sys.stdout.reconfigure(encoding='utf-8')
SINCE = sys.argv[1] if len(sys.argv) > 1 else '2026-09-01'
c = sqlite3.connect("file:C:/Users/terak/AppData/Local/edda/edda.sqlite3?mode=ro", uri=True)
rows = c.execute("""SELECT ts, event, raw FROM events
  WHERE ts >= ? AND event IN ('MissionAccepted','MissionRedirected','MissionCompleted','MissionAbandoned','MissionFailed','Docked','Location','FSDJump','CarrierJump')
  ORDER BY ts, file, offset""", (SINCE,)).fetchall()
docked = None            # (ts, system, station) of the last docked position
system = None
station_system = {}      # station -> system, from every Docked ever seen
M = collections.OrderedDict()
for ts, e, raw in rows:
    v = json.loads(raw)
    if e == 'Docked':
        docked = (ts, v.get('StarSystem'), v.get('StationName')); system = v.get('StarSystem'); station_system[v.get('StationName')] = v.get('StarSystem')
    elif e == 'Location':
        system = v.get('StarSystem')
        if v.get('Docked'): docked = (ts, system, v.get('StationName')); station_system[v.get('StationName')] = system
    elif e in ('FSDJump', 'CarrierJump'):
        system = v.get('StarSystem')
    elif e == 'MissionAccepted' and v.get('KillCount'):
        M[v['MissionID']] = dict(id=v['MissionID'], accepted=ts, giver=v.get('Faction'), target=v.get('TargetFaction'),
            dest_system=v.get('DestinationSystem'), dest_station=v.get('DestinationStation'),
            docked_system=docked[1] if docked else None, docked_station=docked[2] if docked else None,
            accepted_in=system, redirect_ts=None, new_system=None, new_station=None, ended=None, end_event=None, docked_during=set())
    elif e == 'MissionRedirected' and v.get('MissionID') in M:
        m = M[v['MissionID']]; m['redirect_ts'] = ts; m['new_system'] = v.get('NewDestinationSystem'); m['new_station'] = v.get('NewDestinationStation')
    elif e in ('MissionCompleted', 'MissionAbandoned', 'MissionFailed') and v.get('MissionID') in M:
        M[v['MissionID']]['ended'] = ts; M[v['MissionID']]['end_event'] = e
    if e == 'Docked':
        for m in M.values():
            if m['accepted'] <= ts and (m['ended'] is None or ts <= m['ended']):
                m['docked_during'].add(f"{v.get('StationName')} ({v.get('StarSystem')})")

out = io.StringIO(); w = csv.writer(out)
w.writerow(['mission_id','accepted','giver','target','dest_system_at_accept','dest_station_at_accept','dest_station_system_if_known','docked_system_at_accept','docked_station_at_accept','redirect_ts','new_system','new_station','redirect_station_eq_docked_at_accept','dest_station_in_target_system','end_event','docked_during'])
agree = disagree = unknown = 0; dest_in_target = collections.Counter()
for m in M.values():
    eq = None if not m['new_station'] else (m['new_station'] == m['docked_station'])
    if eq is True: agree += 1
    elif eq is False: disagree += 1
    else: unknown += 1
    ds_sys = station_system.get(m['dest_station'])
    in_target = None if not ds_sys else (ds_sys == m['dest_system'])
    dest_in_target[in_target] += 1
    w.writerow([m['id'], m['accepted'], m['giver'], m['target'], m['dest_system'], m['dest_station'], ds_sys, m['docked_system'], m['docked_station'], m['redirect_ts'], m['new_system'], m['new_station'], eq, in_target, m['end_event'], ' | '.join(sorted(m['docked_during']))])
print(out.getvalue())
print(f"# missions {len(M)}; redirect station == docked-at-acceptance: yes {agree}, no {disagree}, no redirect yet {unknown}")
print(f"# DestinationStation-at-acceptance in DestinationSystem (by stations ever docked at): {dict(dest_in_target)}")
print("# distinct (dest_station_at_accept, new_station) pairs:", collections.Counter((m['dest_station'], m['new_station']) for m in M.values()).most_common(8))
