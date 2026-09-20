# ODEliteTracker mission tracking and stacking: gap review

2026-09-20. A written comparison of what
[WarmedxMints/ODEliteTracker](https://github.com/WarmedxMints/ODEliteTracker)
(ODET; C#/WPF, .NET 9, Windows) does for mission tracking and massacre
stacking against EDDA's Missions tab and stacking board, with how each
fact is sourced from the journal and whether EDDA could do the same under
its standing rules. Prompted by the maintainer: "pretty cool app for
mission stacking/tracking. Let's make sure we aren't missing anything."

Read at ODET commit `72cdc1f` (2026-07-14, "Bugfix - Mission lists being
cleared when starting an operation"), cloned read-only. **ODET has no
LICENSE file**, and neither `README.md` nor `ODEliteTracker.csproj`
names a licence; the code is therefore all-rights-reserved by default
and nothing from it was copied into EDDA. This document is a comparison
only. ODET's journal parser lives in sibling projects not in the repo
(`ODJournalDatabase`, `EliteJournalReader`; `ODEliteTracker.csproj`
lines 115-119), so journal field names below are as ODET consumes them
(`accepted.KillCount`, `bountyEvt.VictimFaction`, ...), which mirror the
journal's own names.

ODET paths are relative to its repo; EDDA paths are relative to this one.

## Summary

ODET and EDDA read the same mission lifecycle from the journal
(`MissionAccepted` → `MissionRedirected` → `MissionCompleted` /
`Failed` / `Abandoned`, `CargoDepot` for cargo missions) and both key the
massacre stack on the (issuing faction, target faction) pair. The
differences are:

1. **ODET infers kills from `Bounty`** (one active mission per giver per
   matching kill, capped at `KillCount`). EDDA abandoned this on
   2026-09-19 after measuring it wrong, and this review does not propose
   bringing it back. Roughly a third of ODET's stack numbers rest on it.
2. **ODET computes stack economics that need no inference** and EDDA does
   not: stack value, shareable (wing) value, value ready to turn in now,
   kills needed to clear the stack (max over givers of that giver's
   `KillCount` sum — the concurrent/consecutive rule EDDA already
   records), total kills credited, the ratio between the two, and credits
   per kill. Every input is a stated field of `MissionAccepted` or the
   `MissionRedirected` state.
3. **ODET reconciles against the startup `Missions` event**, dropping
   missions the game no longer lists. EDDA does not read `Missions`; a
   mission that ends without a terminal event in EDDA's journal window
   stays live until its `Expiry` passes.
4. **ODET takes `MissionCompleted.Reward`** (and `Donated` for altruism)
   as the mission's final reward and zeroes failed/abandoned ones; EDDA
   keeps the figure from acceptance.
5. **ODET flags a redirected mission as hand-in-able at the docked
   station** (`Docked.MarketID` against the origin `MarketID`). EDDA shows
   the hand-in station but does not say "you are there".
6. ODET has sortable grids (nine mission sorts, seven stack sorts), a
   completed-missions history with completion times, per-station and
   per-commodity delivery stacks with fleet-carrier stock diffs, and BGS
   influence/reputation accounting from `FactionEffects`. EDDA has a
   fixed HUD order, a history toggle, per-mission delivery bars, and no
   influence handling.

EDDA does several things ODET does not (section below): the hand-in is
the acceptance dock and moves on redirect (ODET never reads
`NewDestinationSystem`/`NewDestinationStation`); redirects are
kind-aware; every mission kind is in one list; the stacking board names
every giver and flags duplicates at the board; voice callouts; an
`Expired` state; and it runs on Windows and Linux.

Recommended top 3, in order: startup `Missions` reconciliation (S),
stack economics without inference (M), and "ready to hand in here" on
dock (S). Reasoning at the end.

## How each side sources missions

### ODET

Three stores each keep their own mission list and each register for the
events they need (`Stores/MassacreMissionStore.cs`,
`Stores/TradeMissionStore.cs`, `Stores/BGSDataStore.cs`).

- **Massacre store** (`Stores/MassacreMissionStore.cs`): parses
  `LoadGame`, `Location`, `FSDJump`, `MissionAbandoned`, `MissionAccepted`,
  `MissionCompleted`, `MissionFailed`, `MissionRedirected`, `Missions`,
  `Bounty`, `Docked`, `Undocked` (lines 26-43). Accepts a mission only
  when a station name is current, `KillCount` is non-null and non-zero,
  `TargetFaction` is set, and `TargetType` contains
  `MissionUtil_FactionTag_Pirate` (lines 73-82) — pirate massacres only.
  Origin system/station/market come from the last `Docked` or `Location`
  (`StationName_Localised`/`StationName`, `SystemAddress`, `StarSystem`,
  `MarketID`; lines 59-68, 199-204) and are cleared by `Undocked`
  (205-208). `MissionRedirected` sets state `Redirected` and forces
  `Kills = KillCount` (95-104). `MissionCompleted` sets `Completed`,
  `CompletionTime` and overwrites `Reward` with `completed.Reward`
  (105-116). `MissionFailed`/`MissionAbandoned` zero the reward (117-136).
  `Missions` (startup) removes any tracked, not-completed mission whose
  `MissionID` is absent from `Active` + `Complete` + `Failed`, unless the
  event looks like an on-foot operation start (`Active.Count == 1`,
  `Active[0].Expires == 0`; lines 137-176). `Bounty` increments `Kills`
  on the first `Active` mission of each issuing faction whose
  `TargetFaction` equals `VictimFaction`, skipping `faction_none`,
  `faction_Pirate`, `suit` targets and `TotalReward <= 0` (177-198). No
  system gate.
- **Mission fields** (`Models/Missions/MissionBase.cs` 29-43, from
  `MissionAccepted`): `MissionID`, `Name`, `LocalisedName`, `Faction`,
  `TargetFaction`, `Influence` and `Reputation` (the journal's `+` strings
  counted via `MissionPlusCount()`, a helper in the external ODMVVM
  project), `Reward`, `Wing`, `Expiry` (defaulting to `timestamp` + 15
  min when absent), `DestinationSystem`, `DestinationStation`,
  `DestinationSettlement`. `Models/Missions/MassacreMission.cs` adds
  `Target`, `TargetType`, `TargetSystem` (= `DestinationSystem`),
  `KillCount`, and a `Kills` setter capped at `KillCount` (19-41).
- **Trade store** (`Stores/TradeMissionStore.cs`): names starting
  `Mission_Collect`, `Mission_Delivery`, `Mission_Mining`,
  `Mission_Altruism` with `Count > 0` (19, 209-225); `CargoDepot`
  `ItemsCollected`/`ItemsDelivered` by `MissionID` (183-194);
  `MissionCompleted` reward = `Reward`, or `-Donated` when `Reward` is 0
  (108). Same `Missions` reconciliation as above (133-172).
- **BGS store** (`Stores/BGSDataStore.cs`): every `MissionAccepted`
  regardless of kind (212-227); on `MissionCompleted` parses
  `FactionEffects[]` (`Faction`, `Influence[]` with `SystemAddress`,
  `Trend`, `Influence`) into signed per-system influence
  (`Models/Missions/FactionEffects.cs` 12-17); `Missions.Failed` marks
  missions failed at the event's timestamp (276-290); `FactionKillBond`
  and `RedeemVoucher` are parsed for BGS, not for missions (101-102).

### EDDA

One derivation over the event log (`crates/ed-store/src/missions.rs`),
rules stated in the module docs (lines 1-32). Reads `MissionAccepted`,
`MissionCompleted`, `MissionFailed`, `MissionAbandoned`,
`MissionRedirected`, `CargoDepot`, `Docked`, `Location`, `FSDJump`,
`CarrierJump` (125-129). Tracks every kind; `kind` is derived from
`Name` (103-107). Fields on `Mission` (56-90): id, accepted, name,
title, faction, kind, target_faction, target, target_type, kill_count,
commodity, count, items_collected/delivered/total, destination_*
(the objective as stated at acceptance), giver_* (the dock at
acceptance), hand_in_* (giver until `MissionRedirected` moves it),
expiry, reward (from `MissionAccepted` only), wing, status, ended.
Status: `Active`, `ReadyToTurnIn` (a redirect on a do-then-return kind,
or `ItemsDelivered >= TotalItemsToDeliver`), `Completed`, `Failed`,
`Abandoned`, `Expired` (expiry before now; 254-266). Kill inference: none
(2026-09-19 ruling, `docs/ROADMAP.md` "Kill counts are gone from mission
tracking"). Startup `Missions`: not read (the only `"Missions"` strings in
`crates/` and `src-tauri/` are a station-service label in
`crates/ed-store/src/lookup.rs:1031` and a journal-noise list in
`src-tauri/src/trade_timing.rs:51`). `Influence`, `Reputation`,
`FactionEffects`, `Donated`: not read anywhere in `crates/ed-store`,
`src-tauri` or `frontend`.

Display: `frontend/src/lib/MissionsPanel.svelte` (columns Mission / For /
Progress / Hand in / Reward / Expires / Status; stats In play, Ready to
turn in, Rewards pending; a history toggle; the stacking board when the
box is ticked), `frontend/src/lib/Overlay.svelte` 222-243 (HUD: three
mission slots in `in_hud_order`, or the stacking board),
`frontend/src/lib/stacking.js` (chip wording), `src-tauri/src/commands.rs`
2284-2297 (`mission_stack`). Stacking board
(`crates/ed-store/src/missions.rs` 331-395): target = the faction with
the most live massacres; every giver against it, alphabetical, with
mission count, ready count and a `duplicate` flag (two or more from one
giver); a count of live massacres against other targets. Callouts:
accepted (with `DestinationSystem`), failed, completed with credits
(`src-tauri/src/callouts.rs` 790-818); per-pass "Mission complete" and
"Mission redirected" (`src-tauri/src/watcher.rs` 586-696).

## Gap table

Effort: S = a day or less, M = a few days, L = a week or more, each
including the measurement CLAUDE.md requires before and the test after.

| # | What ODET does | Journal source (as ODET reads it) | EDDA today | Could EDDA do it under the rules? | Effort |
|---|---|---|---|---|---|
| 1 | Reconciles its mission list against the startup `Missions` event: a tracked mission absent from `Active`+`Complete`+`Failed` is dropped; `Failed` entries are marked failed. Skips the event when it looks like an on-foot operation start (`Stores/MassacreMissionStore.cs` 137-176; `Stores/BGSDataStore.cs` 268-314). | `Missions`: `Active[]`, `Complete[]`, `Failed[]`, each with `MissionID`, `Expires`. | Not read. A mission with no terminal event in EDDA's window stays `Active`/`ReadyToTurnIn` until `Expiry` (`crates/ed-store/src/missions.rs` 125-129, 254-266). | Yes: it is the game stating its own list, exactly the kind of source the 2026-09-19 ruling asks for. Needs the same operation-start guard. | S |
| 2 | Final reward from `MissionCompleted.Reward`; altruism recorded as `-Donated`; failed/abandoned rewards zeroed (`Stores/MassacreMissionStore.cs` 105-136; `Stores/TradeMissionStore.cs` 102-132). | `MissionCompleted`: `Reward`, `Donated`; `MissionFailed`, `MissionAbandoned`. | `reward` is the acceptance figure; `MissionCompleted.Reward` is only spoken (`src-tauri/src/callouts.rs` 807-818), not stored (`missions.rs` 235-238). | Yes. | S |
| 3 | Origin system and origin station as columns, sortable (`Views/MassacreMissionView.xaml` 75-82; `Models/Enums.cs` 98-118). | Last `Docked`/`Location`: `StarSystem`, `StationName(_Localised)`, `MarketID`, `SystemAddress`. | `giver_system`/`giver_station` are on the model (`missions.rs` 80-81) but the tab shows only hand-in, which equals the giver until a redirect (`MissionsPanel.svelte` 94). No sorting. | Yes. | S |
| 4 | Completed-missions history: same columns plus completion time, newest first (`Views/MassacreMissionView.xaml` 603-662; `ViewModels/MassacreMissionsViewModel.cs` 202-204). | `MissionCompleted` `timestamp`. | The history toggle lists every status; `ended` exists on the model (`missions.rs` 89) but is not a column (`MissionsPanel.svelte` 70). | Yes. | S |
| 5 | Per-target stack economics, none of which depend on inferred kills: Stack Value (sum `Reward`), Shareable Value (sum where `Wing`), Current Turn In Value (sum where state is `Redirected`), Shareable Turn In Value, Avg Per Mission, Avg Per Kill (sum `Reward` / kills needed), Mission Count, Active/Redirected counts (`ViewModels/ModelViews/Massacre/FactionStackVM.cs` 24-39; `Views/MassacreMissionView.xaml` 192-363). | `MissionAccepted`: `Reward`, `Wing`, `KillCount`, `Faction`, `TargetFaction`, `DestinationSystem`; `MissionRedirected` for the turn-in subset. | Board shows giver chips with counts, ready and duplicate; the tab has one "Rewards pending" total (`MissionsPanel.svelte` 31-33, 44-63; `stacking.js`). | Yes: every input is a stated field. | M |
| 6 | Kills needed to clear the stack = max over givers of that giver's `KillCount` sum; Total Kills = sum of every `KillCount`; Kill Ratio = the two divided (`FactionStackVM.cs` 27-29, 43-53). This is the concurrent-across-givers / consecutive-within-giver rule in arithmetic. | `MissionAccepted` `KillCount`, `Faction`. | The rule is recorded (`docs/ROADMAP.md` "Mission stacking: the mechanic") and drives the `duplicate` flag, but no number is shown. | Yes: it is a property of the missions held, not of kills made. The maintainer's "no strong modelling" (2026-09-16) was about tracking progress, not about summing targets; confirm before building. | S (with #5) |
| 7 | Stack key includes the target system: (`IssuingFaction`, `TargetFaction`, `TargetSystem`) with a list of origin systems per stack (`ViewModels/MassacreMissionsViewModel.cs` 295-307; `MassacreStackVM.cs` 10-25). | `MissionAccepted` `DestinationSystem`. | Keyed on target faction alone (`missions.rs` 381-385); two stacks against one faction in two systems merge. | Yes. | S |
| 8 | A redirected mission is flagged `AtStation` when the docked `MarketID` equals its origin `MarketID` (`ViewModels/ModelViews/Massacre/MassacreMissionVM.cs` 89-98, driven by `Stores/SharedDataStore.cs` 105, 243, 666). A row style keyed `MasscreMissionRowStyle` exists (`Controls/Styles/DataGridStyles.xaml` 189); which property it triggers on was not verified. | `Docked` `MarketID` vs the origin dock's `MarketID`. | Hand-in station and system are shown; nothing says the commander is standing at it, and no callout on dock (`MissionsPanel.svelte` 94; `callouts.rs` has no `Docked` mission line). | Yes; EDDA already has the dock and the hand-in. Note ODET compares the origin, EDDA should compare `hand_in_*` (which moves on redirect). | S |
| 9 | Sorting and filtering: nine mission sorts (accepted, origin system, origin station, issuing faction, target faction, kills, reward, expiry, wing), seven stack sorts, "hide completed stacks", persisted in settings (`Models/Enums.cs` 98-136; `Models/Settings/MassacreSettings.cs`; `ViewModels/MassacreMissionsViewModel.cs` 61-160). | n/a (presentation). | Fixed HUD order (status, expiry, acceptance; `missions.rs` 298-309); the tab lists in the same order; no controls. | Yes. | S-M |
| 10 | Hovering a stack row highlights that giver's missions in the grid (`Views/MassacreMissionView.xaml.cs` 57-69). | n/a. | Chip hover shows a title string only (`stacking.js` 17-23). | Yes. | S |
| 11 | Relative expiry text refreshed on a timer (5 min for massacre, 1 min for BGS; `MassacreMissionsViewModel.cs` 167, `BGSViewModel.cs` 226). | `MissionAccepted` `Expiry`. | Hours left computed at render (`MissionsPanel.svelte` 27-30); whether the tab re-renders on a timer without a journal event was not checked. | Yes. | S |
| 12 | Separates Odyssey from Horizons/Legacy mission lists using `LoadGame.Odyssey`, and skips reconciliation across the boundary (`MassacreMissionStore.cs` 56-58, 156). | `LoadGame` `Odyssey`. | Not read. | Yes, low value: since Update 14 Live Horizons and Odyssey share the galaxy; only Legacy is separate. | S |
| 13 | Delivery/collect stacks: per origin station, per commodity: mission count, tonnes, delivered, remaining, value, credits per tonne; across all active: totals, turn-in value, shareable value, missions ready (`ViewModels/ModelViews/Trade/*.cs`; `Views/TradeMissionView.xaml` 148-504). | `MissionAccepted` `Commodity(_Localised)`, `Count`, `Reward`, `Wing`; `CargoDepot` `ItemsCollected`, `ItemsDelivered`. | Per-mission delivered bar and "aboard" count only (`MissionsPanel.svelte` 84-87). | Yes. | M |
| 14 | Fleet-carrier stock next to each needed commodity ("C Stock", "C Diff") from the carrier store's `Stock` (`ViewModels/TradeMissionViewModel.cs` 195-208; `Views/TradeMissionView.xaml` 321-337). | Carrier stock: source not traced in this review (ODET's `FleetCarrierDataStore` parses `CarrierStats` etc., lines 98-115; stock may come from its CAPI project `ODCapi`). | No carrier inventory: removed 2026-09-16 because the journal cannot see current stock (`docs/ROADMAP.md` "A carrier's real inventory needs CAPI"). | Only via CAPI, per that ruling. | L |
| 15 | "Ready to turn in" for a delivery mission means the cargo is collected (`ItemsToCollectRemaining == 0`), for collect/mining that it is delivered (`ViewModels/ModelViews/Trade/TradeMissionVM.cs` 94-103). | `CargoDepot` `ItemsCollected`, `ItemsDelivered` vs `Count`. | `ReadyToTurnIn` only when `ItemsDelivered >= TotalItemsToDeliver` (`missions.rs` 209-212); a loaded delivery reads as plain `Active`. | Yes: a "loaded, go deliver" state from the game's own counters. | S |
| 16 | Influence and reputation per mission (count of `+` in the acceptance strings) carried on every mission (`MissionBase.cs` 34-35); on completion, `FactionEffects` folded into per-faction, per-system influence for the current tick, and failed missions counted per faction (`Stores/BGSDataStore.cs` 236-246; `ViewModels/BGSViewModel.cs` 318-345; `ViewModels/ModelViews/BGS/FactionVM.cs` 71-74). The massacre and trade views do not bind Influence/Reputation (no such binding in `Views/MassacreMissionView.xaml` or `Views/TradeMissionView.xaml`). | `MissionAccepted` `Influence`, `Reputation`; `MissionCompleted` `FactionEffects[].Faction`, `.Influence[].SystemAddress`, `.Trend`, `.Influence`. | None of these fields are read. | Yes: faction-level, identifies no commander. Showing `+`/`++`/`+++` per mission is small; tick-scoped BGS accounting is a feature of its own. | S (per-mission) / L (BGS) |
| 17 | BGS Missions tab lists every active mission of any kind: origin, destination (system : station or settlement), issuing and target faction, description, expiry (`Views/BGSDataView.xaml` 487-532; `ViewModels/ModelViews/BGS/BGSMissionVM.cs`). | `MissionAccepted` `DestinationSettlement` in addition to system/station. | Every kind is listed on the Missions tab; `DestinationSettlement` is not read (`missions.rs` 77-78). | Yes. | S |

Not found in ODET, so not gaps: mission-board refresh timers; passenger
mission handling (the word appears only in `Services/NotificationService.cs`,
outside any mission store, and no store filters for `Mission_Passenger`);
courier-specific logic (couriers appear only in the BGS list); any
mission-related notification or toast (`Notifications/` has none; the
Discord summary counts failed missions only, `Helpers/DiscordPostCreator.cs`
99-102); any use of `NewDestinationSystem`/`NewDestinationStation` (no
match anywhere in the repo); expiry warnings beyond relative time.

## What ODET does that EDDA deliberately does not

- **Kill inference from `Bounty`.** ODET's `Kills` (per mission),
  `ActiveKills`, `KillsRemaining`, `KillDifference`/"Diff" (max kills
  across givers minus this giver's; `MassacreMissionsViewModel.cs`
  208-218), `KillsToNextCompletion` (first active mission's
  `KillCount - Kills`, min over givers; `FactionStackVM.cs` 56-61), the
  pop-out's "Kills Next Completion" and "Kills Remaining"
  (`Views/PopOuts/MassacrePopOutView.xaml` 58-67), the "Remaining Kills"
  and "Kills Difference" stack sorts and the "hide completed stacks"
  filter (`KillsRemaining > 0`) all rest on `Bounty` counting. EDDA
  measured this on 2026-09-19 and removed it: a target that dies before
  the scan completes writes no `Bounty` yet counts for the mission (3
  kills in game, 1 in the journal, live); 46 of 65 redirected massacres
  were 2-23 kills short at the redirect
  (`docs/benches/2026-09-19-mission-kill-credit-at-redirect.csv`);
  `docs/ROADMAP.md` "Kill counts are gone from mission tracking". ODET's
  counter additionally has no system gate (a kill anywhere credits) and
  caps at `KillCount`, so it cannot see over-credit either. Not proposed.
  ODET does get one thing right that EDDA also does: `MissionRedirected`
  is the completion signal and forces the count to the target
  (`MassacreMissionStore.cs` 100).
- **Pirate-only massacres.** ODET tracks a massacre only when
  `TargetType` contains `MissionUtil_FactionTag_Pirate`
  (`MassacreMissionStore.cs` 79). EDDA tracks any mission with a
  `KillCount` and a `TargetFaction` (`missions.rs` 369-373). Keep EDDA's.
- **Carrier stock next to delivery needs** (row 14) is off the table
  without CAPI, by the 2026-09-16 ruling.

Nothing in ODET's mission code identifies or locates another commander;
the surveillance rule is not engaged by anything in this table.

## What EDDA does that ODET does not

- The hand-in is the station docked at when accepting and moves only on
  `MissionRedirected` (`NewDestinationSystem`/`NewDestinationStation`;
  `missions.rs` 26-32, 221-233). ODET shows the origin and the
  acceptance-time `DestinationSystem` and never reads the redirect's new
  destination (no `NewDestination` string in the repo), so after a
  redirect its Target System column is where the kills were, not where
  to go.
- Redirects are kind-aware: a courier/delivery/passenger redirect moves
  the drop-off without completing the mission (`missions.rs` 112-119;
  `watcher.rs` 670-696). ODET marks any redirected trade mission
  `Redirected` (`TradeMissionStore.cs` 93-101), though its trade
  ready-state reads the cargo counters instead, so this is harmless there.
- One list for every kind, and an `Expired` status (`missions.rs` 43-54).
- The stacking board is built for the moment of acceptance: every giver
  named, never truncated, duplicates flagged (`missions.rs` 331-395;
  `Overlay.svelte` 222-232). ODET's Diff/Rem columns hint at the same
  thing but nothing says "do not take a second from this giver".
- Voice: accepted, failed, completed with credits, per-pass completion
  naming the giver and count, and reroutes (`callouts.rs` 790-818;
  `watcher.rs` 586-696). ODET has no mission notification.
- Windows and Linux (`docs/BUILDING-LINUX.md`); ODET is WPF on .NET 9.

In fairness, ODET's massacre page is the richer instrument for a
commander mid-stack: two stack grids, a completed grid, sort controls, a
pop-out, and the stack arithmetic. EDDA's is the safer one: nothing on it
is an estimate.

## Recommended top 3

1. **Reconcile against the startup `Missions` event** (row 1, S).
   Correctness first: it is the game's own list, it costs one event, and
   it closes the only way a dead mission can linger on the HUD (no
   terminal event in the window, then days until `Expiry`). Measure
   first, per CLAUDE.md: on the maintainer's store, count missions that
   `active()` returns but the latest `Missions` event does not list; if
   that is zero the row drops to "nice to have" and stays in the table.
   Keep ODET's operation-start guard (`Active.Count == 1`,
   `Expires == 0`) and pin it with a test. Also take `Failed[]` as a
   failure signal.
2. **Stack economics without inference** (rows 5, 6, 7, M). Under the
   giver chips on the Missions tab (and one line on the HUD board): kills
   needed to clear the stack, total kills credited, the ratio, stack
   value, shareable value, and value ready to turn in now, with the
   target system in the key. Every number is a stated field, so it is
   never wrong the way a kill count was, and it answers the question the
   maintainer's twenty-mission stack actually poses at a board: "what is
   this stack worth and how many kills does it cost". Pre-register the
   expected figures from the 2026-09-16 fixture in `missions.rs` (the
   stack of twenty; e.g. kills needed = the largest per-giver sum) before
   writing the function. Confirm with the maintainer that summing targets
   is not the "strong modelling" he declined on 2026-09-16.
3. **"Ready to hand in here" on dock** (rows 8 and 2, S). On `Docked`,
   match `StationName`/`StarSystem` against `hand_in_*` of every
   `ReadyToTurnIn` mission: a pill on the tab and HUD, and one callout,
   "Commander, N missions ready to hand in here, M credits". While
   there, store `MissionCompleted.Reward` (and `Donated`) as the final
   reward so the history and the pending total are the paid figures. Both
   are stated by the game; the callout follows the existing per-pass
   pattern in `watcher.rs` so it speaks once per dock, not once per
   mission. Measure on the maintainer's journal how often a dock
   coincides with ready missions and how many per dock, to size the line.

Rows 9-11 (sorting, hover, timers) are worth doing but are presentation;
rows 13-17 are new surfaces (delivery stacks, BGS influence) and should
wait for a stated need.
