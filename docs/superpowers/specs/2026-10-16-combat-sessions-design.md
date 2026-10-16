# Combat sessions per ship (design, 2026-10-16)

Maintainer's brief: "ship efficiency for combat ... average time from start
of combat to kill for each ship a player has, how long the total combat
session lasted". Approved in chat the same day with one rule added: ships
that never fought do not appear.

## What it is

A derived record of every fight the commander has had, grouped by the ship
that fought it, measured from the journal alone. Nothing comes from the
build or the module stats: this is what happened, not what the fit says
should happen.

## Definitions (pre-registered; the knob and the store must agree)

All from `events` (every journal line). Replay order is `(ts, file, offset)`.

* **Ship at time t**: the last `Loadout` before t (`ShipID`, `Ship`,
  `ShipName` trimmed). Rows group by `ShipID`; the name shown is the last
  one the journal gave that id.
* **Supercruise state**: `SupercruiseEntry`, `FSDJump` and `CarrierJump` put
  the commander in supercruise; `SupercruiseExit`, `Docked`, `Undocked`,
  `Touchdown`, `Liftoff` and `LoadGame` put them in normal space. Nothing
  counts in supercruise: a lock on a passing ship there is not combat, and
  an interdiction's fight starts after the drop.
* **Session = one normal-space instance.** It is entered at
  `SupercruiseExit`, `Undocked`, `Liftoff` or `LoadGame`, and left at
  `SupercruiseEntry`, `FSDJump`, `Docked`, `Shutdown`, `LoadGame`, `Died`,
  or a `Loadout` with a different `ShipID`. A session is kept only if a
  fight happened in it: at least one kill, `UnderAttack`, `HullDamage`
  (PlayerPilot, not Fighter), `FighterDestroyed` or `Died`. Target locks
  alone never make a session (measured 2026-10-16: 274 of 737 gap-based
  runs were lock-only and lasted a median 4 s); nor do shields dropping
  alone, which power management does.
* **Combat events** inside a session: `ShipTargeted` with `TargetLocked`
  (the lock map), `UnderAttack`, kills, `HullDamage`, `ShieldState` down,
  `FighterDestroyed`, `Died`.
* **Session times**: `entered` (the instance start), `start` (first combat
  event), `end` (last combat event), `left` (the closer, null while open).
  "Session length" is `left - entered`, with idle after the last combat
  event capped at 10 minutes: a crash leaves no closer until the next
  `LoadGame`, and an AFK tail is not combat (measured 2026-10-16: one
  Type-10 "session" ran 21 hours). "Fighting" is `end - start`.
  Throughput (kills per hour, credits per hour) is over session length.
* **Kill**: `Bounty`, `FactionKillBond`, `CapShipBond`, `PVPKill`. Target
  ship is `Bounty.Target`, else the ship of the last lock (bonds name no
  ship). Reward is `TotalReward` else `Reward`.
* **Time to kill**: kill ts minus the first lock on that pilot in the
  session (`Bounty.PilotName` matched to `ShipTargeted.PilotName`, raw
  strings; a bond uses the last locked pilot). The lock is consumed by the
  kill (NPC names repeat). A kill with no lock has no time to kill and is
  counted separately: on the Type-10, 1,667 of 2,880 kills had no lock
  and 2,209 came with the fighter out, so they are the fighter's and the
  turrets'. They count for throughput and credits, never for time to kill.
* **Fighter out**: `LaunchFighter` until `DockFighter` or
  `FighterDestroyed`, reset by `Loadout` and by every session closer (a
  fighter cannot follow the ship out of the instance).
* **Kill interval**: kill ts minus the previous kill in the session, or
  minus `start` for the first.

`Music` tracks are not used: the journal only started writing them on
2026-10-15 for this commander.

## Storage (ed-store)

New table `combat_sessions` (one row per kept session; `id` autoincrement;
`ship_id`, `ship`, `ship_name`, `entered_ts`, `start_ts`, `end_ts`,
`left_ts` NULL while open, `closed_by`, `kills`, `kills_timed`,
`kills_fighter`, `credits`, `under_attack`, `hull_hits`, `shields_down`,
`died`, `system_name`, `start_file`, `start_offset`). Four columns added
to `combat_kills`: `session_id`, `ttk_s`, `interval_s`, `fighter_out`.

Derivation `combat::derive_sessions(conn)` runs after every
`derive_incremental`/`derive_all`. It is idempotent by construction: it
finds the last *closed* session, deletes everything after it (the open
tail, at most the current instance), restores the state at that point
(ship from the last `Loadout`, supercruise from the closer) and replays
from there. A fresh store replays everything once.

## Queries

* `combat::by_ship(conn, since)`: one row per ship with at least one
  session in the window: sessions, hours, kills, kills timed, fighter
  kills, kills per hour, median and p90 time to kill, median interval,
  credits, credits per hour, hull hits, shields down, deaths.
* `combat::sessions(conn, ship_id, since, limit)`: the session list for
  one ship, newest first, the open one first.

## UI

Combat tab gains "By ship" (the table above, ranked by kills, only ships
that fought) and, on picking a ship, its sessions. The latest-kills table
gains a time-to-kill column (blank for fighter and turret kills). Every
panel change gets an SSR render test with real data.

## Measurement

`docs/benches/knobs/combat_sessions.py` is the reference implementation of
the definitions; it reads the store or a JSONL fixture. A fixture of real
events (one day of the maintainer's journal, NPC names only) pins the Rust
derivation to the knob's numbers: session count, kills, timed kills and the
median time to kill must match exactly. Per-ship rows pinned in
`docs/benches/2026-10-16-combat-sessions-per-ship.csv`.

## Not in scope

Theorycraft (DPS, time-to-kill from the build), comparison against other
commanders (EDDA is not a surveillance tool: PVP victims are counted and
never named), and anything from `Music`.
