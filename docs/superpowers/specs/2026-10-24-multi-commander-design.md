# Multiple commanders: one EDDA, one store per account, one HUD

Design, 2026-10-24. Status: APPROVED 2026-10-24; implemented on branch
`multi-commander` (plan `docs/superpowers/plans/2026-10-24-multi-commander.md`).
The baseline is measured and on record
(`docs/benches/2026-10-24-two-account-folder-baseline.csv`); the replay
verdict is `docs/benches/2026-10-24-multibox-replay.csv`.

## The ask (maintainer, 2026-10-24)

"A feature that people have been asking us to support: multiple
accounts / multiboxing." Shape ruled in conversation the same day:

- one EDDA process, a window or tab per commander, **one HUD** that shows
  whichever commander the player has selected ("two huds won't make
  sense");
- selection is **manual** for the first release ("let's be safe");
- the unselected commander may speak **danger only**, off by default;
- partition by **one store per commander** ("this is ultimately safer
  anyway").

## What the donor's journals say

A player with two accounts sent their Saved Games folder (1,396 journal
files, 2025-01 to 2026-10). It stays on the maintainer's machine; nothing
from it enters the repo but counts and shapes, commanders labelled A and
B. Knobs: `docs/benches/knobs/multibox/` (survey, flips, ambiguity,
balance), each taking the folder or the replayed store as its argument.

| Fact | Count |
|---|---|
| Distinct FIDs in one folder | 2 |
| Files carrying events of two FIDs | 0 |
| Files with no Commander event (launcher opened, quit at the menu) | 164 |
| Lines before the Commander event in a file, worst case | 45 (Friends, Music) |
| Commander events repeated inside one file (relog to menu and back) | 1,492 |
| Session pairs with both accounts in game at once | 141 |
| Hours and days of concurrent play | 277 h over 69 days |
| Concurrent pairs with both accounts in the same system | 52 |
| Events written within 1 s of the other account's | 29,952 |
| ShipIDs used by both commanders | 16 |
| Owned carriers | both accounts |

Consequences:

1. **The journal file is the unit of attribution.** A file belongs to one
   FID for its life; its FID is known by its 46th line at the latest.
2. **Concurrent play is the normal case, not the edge.** Both instances
   write the same folder at the same second, often from the same system,
   often in a wing with each other.
3. **ShipID collides across accounts**, so any table keyed by ShipID
   alone blends two ships.
4. **The companion files carry no FID.** Status.json, Cargo.json,
   NavRoute.json and the rest are one set per Windows user, overwritten by
   whichever instance wrote last.

## What EDDA does with that folder today (measured)

The folder replayed through the shipped store (`ed-store` example
`first_sync`, 2 min 30 s, 1.49 GB). All 1,257,243 events land in one
`events` table with no commander column; every reader takes the newest
row by `(ts, file, offset)`.

| Reader ("newest row wins") | Commander flips, all time | Last 90 days |
|---|---|---|
| Commander (the name the app greets) | 469 | 263 |
| Loadout (the ship) | 542 | 312 |
| Cargo | 539 | 287 |
| Missions | 473 | 265 |
| Materials | 469 | 263 |
| Location | 475 | 267 |
| ShipLocker | 881 | 519 |
| CarrierStats | 77 | 39 |

590 state-bearing rows arrived within 60 s of the other commander's. The
missions table holds both accounts' missions as one list; the carriers
table holds both carriers; 16 ships merge by ShipID. A multiboxer running
EDDA today sees a blend that changes several times an hour.

Companion attribution by timing was measured and buried: with "the
commander whose journal wrote most recently" the companion write is
ambiguous for 15.6% of state lines at a 3 s window (8.9% at 1 s, 31.8% at
10 s). By content it is not: the two accounts' credit balances are never
close (figures withheld: balance-derived; bench CSV) and in 133 of 141 concurrent
windows neither side's turnover reached half the gap; cargo tonnage ties
in 70% of concurrent minutes, but a tie is harmless because the values
are equal.

## The design

### 1. Store and ingest

- **One SQLite file per commander** beside today's `edda.sqlite3`:
  `edda.<FID>.sqlite3`. The galaxy file stays shared. At first launch the
  existing `edda.sqlite3` is renamed to the FID its Commander events name
  (one FID in a single-account store; a blended store from a two-account
  folder is re-ingested from the journal, which is what the store is for).
  Current users migrate nothing by hand and see no change.
- **The folder is watched once.** `ingest_all` and the watcher's sync
  read each file's lines until the first `Commander` or `LoadGame` with
  an `FID`; the whole file, including the lines before it, is routed to
  that FID's store. A file is held, not ingested, until its FID is known.
  A file whose later lines name a second FID is refused from that line
  with one warn log line, never blended (test; the donor's 1,396 files
  never did it).
- **Menu-only files** have no FID and nothing of value: ingested nowhere,
  listed once at info level with their count.
- **Companion files are attributed by content, never by timing:**
  Status.json by nearest `Balance` to each store's tracked credits
  (LoadGame `Credits` plus the money events since); Cargo.json by
  inventory equality with each store's latest `Cargo` event; Market,
  Outfitting, Shipyard by `MarketID` against each commander's latest
  `Docked`; FCMaterials by `CarrierID` against each store's carrier;
  ModulesInfo and ShipLocker by content equality with the latest
  `Loadout` / `ShipLocker`; NavRoute by its first hop against each
  commander's current system. A file matching exactly one store goes
  there. A file matching two stores with equal content goes to both (the
  values are the same). A file matching none, or two with differing
  content, is written to no store and the `snapshots` row of every store
  is marked `unknown`, which the fuel gauge, flags and nav target show as
  such. The 8 close Balance windows in the donor data are the harness's
  first report.
- **ShipID stays per store**, so the collision disappears by
  construction. The `ships` and `ship_locations` tables are unchanged.

### 2. App layer, selection, HUD, voice

- The `Arc<Mutex<Store>>` handle becomes a registry: `Stores { by_fid:
  BTreeMap<Fid, Store>, selected: Fid }`, with `selected` persisted in
  settings. At launch the selection is the commander whose journal wrote
  last, or the only one. A new FID appearing mid-session is opened and
  added without a restart.
- Every Tauri command that reads the store reads `stores.selected()`. The
  queries do not change; only the handle they are given.
- Selecting emits the existing journal-changed signal; every panel
  re-derives from the other store. Panels learn nothing about commanders.
- The HUD overlay and the voice are bound to the selected commander. For
  each unselected commander a watcher pass still runs its callout
  pipeline but only the **danger set** reaches the voice, prefixed with
  that commander's name: under attack, interdicted, hull below 25%,
  fighter destroyed, carrier jump in 60 s, docking denied. Behind
  Settings > Experimental > "Alerts from the unselected commander" (the
  toggle began under Voice; the maintainer, 2026-10-24: the feature ships
  marked experimental, so its controls live together there), shipped off. The set
  is a constant with a test; nothing else from an unselected store is
  spoken.
- The Frontier link stores one refresh token per FID
  (`frontier_refresh_token:<FID>`); the existing slot migrates to the
  migrated store's FID. `fid_matches` compares against the selected
  commander.
- Route following and the key macro are unchanged: keys go to the
  foreground game as now; the player focuses the instance they fly. The
  active route lives in the selected store, so each commander carries
  their own.
- EDDN upload reads each store's own watermark. Telemetry carries no FID,
  no name and no commander count (closed allowlist). The server sees
  nothing new.

### 3. Frontend

- A **commander strip** above the existing tabs, rendered only when the
  registry holds more than one FID: one pill per commander with name,
  ship, system, and a dot when that account's journal wrote in the last
  minute. Click selects. With one commander nothing renders and the app
  is today's.
- The strip is the only new component. Two new commands, `list_commanders`
  and `select_commander`, registered in `api.js` table and export list.
- Settings > Voice gains the danger-alerts toggle; Settings gains a
  read-only list of the commanders found with their store sizes.
- Onboarding gains one line when two FIDs are found on first run: which
  commander is selected and where to switch.
- Copy: "Commander" only; the other account named by its commander name
  in danger lines; nothing on screen is another EDDA user, only the
  player's own accounts.
- Tests: an SSR render test of the strip with two commanders and with
  one; the `api.js` contract test covers the two commands.

### 4. Measurement, rollout, scope

- **Baseline on record** before code:
  `docs/benches/2026-10-24-two-account-folder-baseline.csv`.
- **Replay harness:** the donor folder through the new ingest into a
  scratch directory, verdict pre-registered as zero rows of A in B's
  store and the reverse, zero flips on every reader above, Balance
  attribution unambiguous in 133 of 141 windows with the 8 close ones
  listed, menu-only files 164 and ignored.
- **Sync cost pre-registered as unchanged:** two stores ingesting the same
  bytes cost what one does today within the `sync_cost` example's noise
  band; more than 10% slower on the donor folder fails the pin.
- **Existing users see nothing:** one FID, no strip, one rename. Tested
  on a copy of a single-commander store and on the two-account replay.
- **Flight before ship**, on a real two-instance machine; the donor is the
  natural tester, with the harness's log lines as the report.
- **Out of scope, OPEN in the roadmap:** selection following the focused
  game window (the matching exists on paper: process start time against
  Fileheader timestamp, unverified on a real machine); a both-at-once
  wing view; separate journal folders on one machine; two PCs sharing one
  EDDA.
- **Risks named:** a future game build writing two FIDs into one file
  (refusal test); Balance drift in a long session with a near-equal gap
  (the unknown state, never a wrong gauge); keychain slot naming on Linux
  (CI on both platforms).

## Open questions

- Whether the `snapshots` table should move out of the per-commander
  stores into a shared file with an `fid` column, since one companion
  write can belong to two stores. Decided at plan time by whichever keeps
  the readers unchanged.
- Whether the danger set should include "low fuel" for the unselected
  commander. Left out: it is not sudden, and the selected view shows it.
