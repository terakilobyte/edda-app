# Fleet carriers in EDDA — design (proposal, awaiting maintainer ruling)

Status: PROPOSAL 2026-09-06, review. Nothing here is built. The maintainer
rules in the assistant's session; this document is the thing being ruled on.

## 0. Why now, and what was measured first

Discord users asked for (a) fleet-carrier route planning and (b) the
ship computer to know the carrier's Tritium. Today it answers:

> "I do not have the carrier hold. Journal tools cover your ship and
> personal materials only." — `get_inventory`, `get_ship_status`

Measured before designing (doctrine rule 1):

| Instrument | Result |
|---|---|
| Prod feedback table (2026-09-06) | 4 rows, all wire tests. The demand is Discord-only. |
| Codebase grep for `Carrier*` handling | Only `CarrierJump`, treated as a location synonym. No `CarrierStats`, `CarrierLocation`, `CargoTransfer`, `CarrierDepositFuel`, `CarrierTradeOrder` derivation. `FCMaterials.json` not in `COMPANION_FILES`. |
| A journal `events` table (review, read-only) | `CarrierLocation` ×23 (nightly heartbeat since 2026-08-28), `CarrierStats` ×1, `CargoTransfer` ×1. **Raw ingest already banks every carrier line** (`crates/ed-store/src/ingest.rs:165` has no event allowlist). |
| Ownership check | The `CarrierStats` row is a **squadron** carrier, not an owned one. `CarrierStats` presence ≠ ownership. |
| Mac dev DB | 0 events (no Elite here). Repo has no carrier fixtures. |

Conclusion: the first user-visible win is a **derivation** over data
already on disk, not a new data source. CAPI is the second win and is
owner-only.

## 1. Source facts (EDCD / Frontier / Journal Manual v38)

Sources: EDCD `FDevIDs/Frontier API/*.md`, EDMC `companion.py`,
EDDiscovery `CAPI/*.cs`, Journal Manual v38, ed-math carrier page.
Working copies in the session scratchpad; citations in the ledger entry.

**Auth.** OAuth2 authorization-code + PKCE (S256), public client, no
secret. Authorize `https://auth.frontierstore.net/auth`
(`response_type=code&audience=frontier,steam,epic&scope=auth capi&
client_id&code_challenge&code_challenge_method=S256&state&redirect_uri`).
Token `POST https://auth.frontierstore.net/token` (form-encoded), grants
`authorization_code` and `refresh_token`. Decode `GET /decode` returns
`customer_id` (journal `FID` = `F{customer_id}`; EDMC checks this to
catch a wrong-account login). Access token 4 h (`expires_in` 14400).
Refresh tokens rotate on every refresh and die **25 days after the
initial authorization** — the user must re-link monthly, by Frontier
design. **One active authorization per (account, client ID)**: linking
on a second PC silently unlinks the first. Expired access → HTTP 401.
Required `User-Agent` shape: `EDCD-EDDA-<version>`.

**Client ID.** The maintainer already holds one (registered at
`user.frontierstore.net` Developer Zone). Public-by-design under PKCE,
but EDDiscovery's condition ("not allowed to check the client ID into
version control") is worth honouring: inject at build time via
`option_env!("EDDA_CAPI_CLIENT_ID")`. When absent the Link button does
not render and the code path is dead — dev builds stay honest.

**Redirect URI.** EDMC (Windows) and EDDiscovery use a custom scheme
(`edmc://auth`, `eddiscovery://auth/`); EDMC on Linux uses
`http://localhost:<random port>/auth`. EDCD's own example is
`myapp://fd-auth-redirect`. Whether Frontier special-cases `localhost`
is inferred from EDMC behaviour only — unverified for a new client.

**Hosts.** Live `https://companion.orerve.net`; Legacy
`https://legacy-companion.orerve.net`. Pick by journal `gameversion`
≥ 4.0 (EDMC `is_live_galaxy`). No legacy `/fleetcarrier`.

**`/fleetcarrier`.** `200` = data, **`204 No Content` = commander owns
no carrier** (EDMC mis-handles this as a server error; we will not).
Shape: `name{callsign, vanityName (hex UTF-8)}`, `currentStarSystem`,
`fuel` (**string**, tank Tritium, max 1000), `balance` (string),
`state`, `dockingAccess`, `notoriousAccess`,
`capacity{shipPacks, modulePacks, cargoForSale, cargoNotForSale,
cargoSpaceReserved, crew, freeSpace, microresource*}`,
`itinerary{completed[], totalDistanceJumpedLY, currentJump}`,
`finance{...}`, `servicesCrew{...}`, **`cargo[]` one entry per unit**
(`{commodity, mission, qty=1, value, stolen, locName}`) — a full hold is
tens of thousands of entries, which is why EDMC gates the call behind
an off-by-default option, fires it only on `CarrierBuy`/`CarrierStats`,
enforces a 15-minute cooldown and a 60 s timeout, and documents calls
"sometimes requiring up to 20 minutes". `orders{commodities{sales,
purchases}}`, `carrierLocker`, `market{commodities[]}`, `ships`,
`modules`. Frontier: "not designed as a real-time service", ≤ 1 query
a minute, rate-limited above 2/s.

**Journal (owner-independent).** All `Carrier*` events carry
`CarrierID` and, since Aug 2025, `CarrierType` ∈ {`FleetCarrier`,
`SquadronCarrier`}.
- `CarrierStats` — "when owner opens carrier management" (also written
  for a squadron carrier by an officer): `Callsign, Name, FuelLevel:int,
  JumpRangeCurr/Max, SpaceUsage{TotalCapacity, Crew, Cargo,
  CargoSpaceReserved, ShipPacks, ModulePacks, FreeSpace}, Finance{...},
  DockingAccess, AllowNotorious, PendingDecommission`.
- `CarrierLocation` (Trailblazers U1, 2025-03-06, not in v38 PDF;
  community schema): "on startup and after a carrier jump" —
  `StarSystem, SystemAddress, BodyID`. The reliable heartbeat.
- `CarrierJumpRequest{SystemName, SystemAddress, Body, DepartureTime}`,
  `CarrierJumpCancelled`, `CarrierJump` (when aboard).
- `CarrierDepositFuel{Amount, Total}` — `Total` is the tank after
  deposit; fires for any depositor.
- `CarrierTradeOrder{Commodity, PurchaseOrder|SaleOrder|CancelTrade,
  Price}`; `CargoTransfer{Transfers[{Type, Count, Direction:
  tocarrier|toship|tosrv}]}`.
- `FCMaterials.json` — bartender micro-resources only. Absent on the
  maintainer's box; presence is state-dependent (written when the bartender
  screen opens), not proof of nonexistence.

**Ownership signal.** A personal fleet carrier is only ever managed by
its owner, so `CarrierStats` with `CarrierType == "FleetCarrier"`
implies ownership; `SquadronCarrier` does not. `CarrierBuy` is the
strong signal. `/fleetcarrier` `204` is the definitive negative.

**Measured on a real carrier (2026-09-06, census in the
ledger):** (a) `CarrierNameChange` writes `CarrierType` under an
EMPTY-STRING key — a Frontier bug; never read the type from that
event, carry it by `CarrierID`. (b) `CarrierJumpRequest.DepartureTime`
runs ~30 min ahead of the jump: plannable, so A's callouts count down
and C's follow mode warns before lockout. (c) `CarrierID` equals the
`Docked`/`CarrierJump` `MarketID` and survives a rename with the
callsign; the vanity name does not — nothing keys on name. (d) A new
carrier ships with Captain, Commodities and CarrierFuel crew and 500 t
of tritium. (e) Squadron callsigns are four characters, not XXX-XXX;
`CarrierType`, not callsign shape, decides.

**Jump physics.** Max 500 ly. Tank 1,000 t. Capacity 25,000 t. Cannot
jump to permit-locked systems regardless of the owner's permits.
Timing: ≥ 15 min spool, lock at 3:20, 5 min cooldown ≈ 20 min/jump.
Fuel per jump (ed-math, community-accepted):

    fuel_t = round(5 + d_ly × (capacity_used + tank_tritium + 25000) / 200000)

Tritium in the tank counts as mass. Min 5 t, max 133 t (full carrier,
500 ly). Spansh's router takes `capacity_used, fuel_loaded,
tritium_stored, refuel_destinations` and assumes 20 min/jump and
150 t/h mining. No "no-body system" rule was found in any source.

## 2. Scope: three sub-projects, not one

This request spans three independent subsystems. Each gets its own
spec → plan → build cycle. This document fixes the boundaries and the
order; only sub-project A is designed to build-ready depth here.

| | Sub-project | Data source | Works for | Blocked on |
|---|---|---|---|---|
| **A** | Carrier status from the journal | `events` rows already ingested | own carrier, **squadron carrier**, carrier you are docked at | nothing |
| **B** | Frontier link (CAPI) | `/fleetcarrier`, `/profile` | **owned carrier only**: hold contents, orders, finance, remote state | a carrier-owning tester (the maintainer does not own one) |
| **C** | Carrier route planner | `ed-galaxy` index (+ A/B for mass and fuel) | anyone planning jumps for any carrier | A (for the mass inputs), a pinned-route bench |

Order: **A now** (unblocked, answers the Discord question honestly, and
*measures* how stale journal-only carrier data is — that number is what
justifies B). **B** starts once a tester is found; the OAuth plumbing
can be built and tested against `/profile` on a test account
meanwhile. **C** after A; its bench is pre-registered below.

## 3. Sub-project A — carrier status from the journal (BUILT 2026-09-07; ledger "Item 52 A BUILT")

### 3.1 Storage: one derived table

New `ed-store` derivation batch (`derive::DERIVED_FROM`,
`crates/ed-store/src/derive.rs:19`) over the events already in
`events.raw`:

```
carriers(
  carrier_id      INTEGER PRIMARY KEY,   -- CarrierID
  carrier_type    TEXT NOT NULL,         -- FleetCarrier | SquadronCarrier
  callsign        TEXT, name TEXT,
  owned           INTEGER NOT NULL,      -- 1 iff CarrierBuy seen OR (CarrierStats AND type=FleetCarrier)
  system_name     TEXT, system_address INTEGER, body_id INTEGER,
  location_ts     TEXT,                  -- newest of CarrierLocation / CarrierJump / CarrierJumpRequest(arrived)
  fuel_t          INTEGER, fuel_ts TEXT, -- newest of CarrierStats.FuelLevel / CarrierDepositFuel.Total
  capacity_total  INTEGER, capacity_used INTEGER, free_space INTEGER, stats_ts TEXT,
  jump_range_curr REAL, jump_range_max REAL,
  docking_access  TEXT, pending_decommission INTEGER,
  balance_cr      INTEGER,               -- Finance.CarrierBalance (squadron rows too)
  pending_jump_system TEXT, pending_jump_ts TEXT, pending_departure TEXT  -- from JumpRequest until Jump/Cancelled
)
carrier_hold(carrier_id, commodity TEXT, count INTEGER, ts TEXT, PRIMARY KEY(carrier_id, commodity))
  -- running sum of the COMMANDER'S OWN CargoTransfer moves (tocarrier +, toship/tosrv −), never negative.
  -- Honest label: "what you moved", not "what is aboard".
```

Every value carries the timestamp it was true at. The tool reports the
age. That age is the measurement (§3.5).

`FCMaterials.json` (bartender) is **out of A's scope** (maintainer,
2026-09-06: "don't need bartender for now"); it stays a one-line
`COMPANION_FILES` addition for whenever the bartender is wanted.

### 3.2 Ship computer tool

One `ToolSpec` row in `src-tauri/src/capabilities/tools.rs` →
`capabilities/carrier.rs::status()`:

`get_carrier_status` — "The commander's own or squadron carrier as the
journal last saw it: location, Tritium in the tank, capacity, pending
jump, and what the commander has moved into the hold. Every figure
carries its age. Does not know other players' deposits or sales;
if the commander has linked Frontier, prefer the live figures."

Returns `{carriers: [{callsign, name, type, owned, location{system,
as_of}, tank_tritium{t, as_of}, capacity{total, used, free, as_of},
pending_jump{system, departure}, hold_moved[{commodity, t, as_of}]}],
note}`. Empty list → the tool says so, with
the reason ("no Carrier events in the journal yet — open Carrier
Management once and ask again").

The answer to the Discord question after A:

> Tank: 1,000 t Tritium as of 28 Aug, when you last opened carrier
> management. Hold: you moved 0 t of Tritium aboard since the journal
> began; other commanders' deposits are invisible to me until you link
> Frontier.

An `eval.rs` case pins it (`expect_tools: get_carrier_status`,
`forbid_tools: get_inventory` for the carrier question).

### 3.3 Callouts (small, opt-in flag `carrier_callouts`)

`CarrierJumpRequest` → "Carrier jump to <system> scheduled, departure
in N minutes"; `CarrierJumpCancelled`; `CarrierLocation` change while
not aboard → "Your carrier has arrived at <system>". Phonetics reuse
`phonetics.rs`. Muted under test like everything else.

### 3.4 UI

No new tab in A. The existing Ships tab gains a "Carrier" card when the
`carriers` table is non-empty (callsign, location + age, tank + age,
pending jump). A dedicated tab waits for B/C, which have content that
needs one.

### 3.5 Measurement (rule 2: ships measurable)

Local numbers, never shipped: `carrier_events_derived_total{event}`
trace at derive time; tool result includes `as_of` ages.

Shipped (allowlist, closed set): feature flag `carrier_callouts`;
`record_timing("carrier_derive", ms, ok)` added to the `debug_assert`
set and the server boundary; one search-kind style number
`carrier_stats_age_hours` sampled when `get_carrier_status` runs — a
bare u32, pooled, so we learn *how stale journal-only carrier data is
across users*. That histogram is the pre-registered instrument for B:
**if p50 age < 24 h, B's value is the hold contents, not freshness; if
p50 age > 72 h, B is a freshness fix too.** Privacy page gains one line
for the new number (same paragraph as the two search settings).

### 3.6 Tests

- Derivation fixtures: synthetic `CarrierStats` (both `CarrierType`s),
  `CarrierLocation`, `CarrierJumpRequest`→`CarrierJump`,
  `CarrierJumpRequest`→`CarrierJumpCancelled`, `CarrierDepositFuel`
  newer than `CarrierStats`, `CargoTransfer` in both directions with
  the floor at zero. One test per finding, failing-then-passing.
- Ownership: `SquadronCarrier` stats → `owned=0`; `CarrierBuy` → 1.
- Tool contract test already asserts definitions and runners match.
- Telemetry anonymity test extended for the new kind/number.

## 4. Sub-project B — Frontier link (design to approach depth)

### 4.1 Architecture ruling being asked

**Client-side only** (the assistant's recommendation; mine too): PKCE in the
app, tokens in the OS keychain (`state::secrets`, slot
`frontier_refresh_token`, zero new deps), CAPI fetched by the app over
the shared `reqwest` client on `AppState` (the contract test in
`contracts.rs:80` forbids a second client). **Nothing from CAPI ever
reaches ed-api**: commander name, credits, position, cargo, finance
all stay on the machine. Telemetry may carry `record_timing("capi",
ms, ok)` and a `capi_linked` feature flag, nothing else; the anonymity
test pins it. The server never holds a token — the surveillance law
makes this the only shape that can ship.

Named carve-out (the assistant session): *offline* carrier monitoring (alerts while
the app is closed) would require server-held tokens and positions. It
is **out of scope** and, if ever raised again, is its own feature-shape
question under the law, not an extension of B.

Audience string: EDMC sends `audience=frontier,steam,epic` so that
players whose game account is a Steam or Epic login can complete the
Frontier auth page; we do the same. `/decode` `customer_id` vs journal
`FID` is the check that the login matched the game account.

### 4.2 Redirect — MEASURED 2026-09-06: direct `edda://auth`, trampoline UNBUILT

Pre-registered spike (d46cedd, review; ran 2026-09-06 on a real
account, Windows dev build): Frontier accepts
`redirect_uri=edda://auth` and Windows delivers it into the running
app.

| Call | Status | ms |
|---|---|---|
| `/token` (PKCE exchange) | 200 | 748 |
| `/decode` (`customer_id` == journal FID) | 200, match | 185 |
| `/profile` | 200 | 985 |
| `/fleetcarrier` (owns no carrier) | **204** | 567 |

The state check passed against the running instance's in-memory
pending state, so the callback reached the live app: no second
instance. Tokens were dropped with the task.

Consequences:
- **The https trampoline is not built.** The registered
  `https://api.edda-app.com/v1/auth/frontier/callback` URI stays on
  the client as insurance and goes unused; ed-api gains no auth
  endpoint. The server's role is galaxy-side routing only.
- The app registers `edda://auth` via `tauri-plugin-deep-link` +
  `tauri-plugin-single-instance` (feature `deep-link`), scheme in
  `tauri.conf.json` `plugins.deep-link.desktop.schemes`,
  `deep-link:default` in `capabilities/default.json`; NSIS registers
  it on Windows, `.desktop` on Linux, `register_all()` in debug builds.
- The Settings "Frontier account" section keeps a "Paste the code from
  the browser" box beside Link for the edge case of no scheme handler
  at redirect time; the state check applies to a pasted code exactly
  as to a deep-linked one. UI copy says "your browser may ask
  permission to open EDDA".
- Loopback `http://localhost:<port>` is dropped.
- **Unmeasured: Linux and macOS deep-link delivery** (AppImage
  relocation is the known risk). A per-platform re-run of the spike is
  a gate before B ships to the Linux alpha testers.

**Ruled with it (maintainer, 2026-09-06): the server never queries CAPI.**
Every CAPI call is made by the app, with the player's token, over the
player's connection.

### 4.3 Fetch policy (learned from EDMC's scars)

- Off by default; a Settings → Application → "Frontier account" section
  with Link / Unlink, the 25-day re-link explained in the UI copy, and
  "linking on another PC unlinks this one".
- `/profile` on link (cheap) → verify `/decode` `customer_id` == journal
  `FID`; mismatch = "that Frontier login is not the account this
  journal belongs to", tokens discarded.
- `/fleetcarrier` only when `owned=1` per §3.1 **or** on first link
  (a `204` sets `owned=0` definitively and stops further calls). Then
  on `CarrierStats`, `CarrierBuy`, `CarrierTradeOrder`,
  `CarrierDepositFuel`, and manual refresh; 15-min cooldown; 120 s
  timeout on its own task so it never blocks `/profile`; result cached
  in a `carrier_capi` table with `fetched_at`; `cargo[]` folded to
  `{commodity: count}` before storage (one entry per unit on the wire).
- Host by journal `gameversion`; Legacy has no `/fleetcarrier`.
- 401 → refresh; refresh 401 → "link expired, re-link" state, never a
  modal; 418/5xx → backoff, quiet.

### 4.4 What B adds to the tool

`get_carrier_status` gains `live: {fetched_at, tank_tritium,
hold[{commodity, t}], orders, balance, state, docking_access,
itinerary.current_jump}` and prefers it when fresher than the journal
figure, saying which it used.

### 4.5 Blockers

- A carrier-owning tester. The test account returns `204`. OAuth,
  `/profile`, `/decode`, keychain, re-link, and the `204` path can all
  be built and flown on a test account; the `200` path needs a volunteer
  from Discord who is willing to run a dev build.
- Empirical: `/fleetcarrier` latency and payload size on a real full
  hold; freshness semantics (live vs on-event). First thing to measure
  with the tester, before the fetch policy is finalised.

## 5. Sub-project C — carrier route planner (design to approach depth)

### 5.1 Where it runs — follows the data-residency ruling (2026-09-05)

The ledger's residency classes settle this without a new ruling:
carrier state (sub-projects A and B) is **personal** → always local,
never API. Carrier routing is **galaxy core** → local-by-choice with
API backup, exactly like the ship router already is after "The
data-source choice: Use remote API / Use local data" (d1210f5).

So C is the same algorithm in two homes, behind the existing choice:

1. **Local, in `ed-galaxy`** — `carrier_plan(g, req, ctl)` beside
   `router::plan` (`crates/ed-galaxy/src/router.rs:363`): fixed 500 ly
   reach, no supercharge, no scoop, cost = jumps then fuel, over the
   existing spatial grid. Runs when the routing index is resident.
   Permit-locked systems: a static list in `ed-domain` (few; change
   with patches, not daily).
2. **Server, `POST /v1/carrier/route` in ed-api** — the same function
   over the server's galaxy for thin clients ("Use remote API"). The
   request carries only what the ship-route endpoint already carries
   (endpoints, mass, fuel numbers); the server answers and forgets.
   Ships with `edda_carrier_route_*` series and a dashboard row the
   same day, per the standing directive.
3. Spansh's undocumented job API (PLAN.md row): rejected — an external
   dependency for a computation we own, against their stated
   preference.

Build order inside C: the pure planner first (one crate function, one
bench), then both homes wire it. The bench (§5.3) runs against the
crate function, so both homes are pinned by the same CSV.

### 5.2 Inputs and outputs

Request: `from`, `to`, `capacity_used` (from A/B, editable),
`tank_tritium` (A/B, editable), `hold_tritium` (B or typed),
`refuel_policy` (none | stop at tritium sellers from the market DB |
mine). Response: hops `[{system, distance_ly, fuel_t, tank_after}]`,
totals (jumps, fuel, ETA at 20 min/jump), and a "short on fuel by N t
at hop k" verdict when the tank runs dry — never a silently truncated
route.

### 5.3 Pre-registered bench (rule 4)

`docs/benches/carrier-router.csv`, knobs in `docs/benches/knobs/`.
Pins: Sol → Colonia, Sol → Beagle Point, a 1,200 ly bubble hop, a
permit-adjacent case (must route around it). Expected: jump counts
within ±1 of Spansh's published router for the same inputs; fuel
totals within rounding of the ed-math formula. A pin that worsens
fails the build.

### 5.4 Follow mode

Reuses the route-following block: on `CarrierJumpRequest` matching the
next hop, mark it; on `CarrierLocation` arrival, advance and speak the
next hop and its fuel. The HUD block says "CARRIER ROUTE".

## 6. Privacy page and release notes

- Privacy: new section "Linking your Frontier account (optional)": what
  the link grants, that tokens live in the OS keychain, that carrier
  and commander data never leave the machine, that our server never
  calls Frontier on your behalf and only ever relays a one-time login
  code it cannot redeem (the secret that redeems it never leaves your
  PC), and the two new pooled numbers (carrier-stats age; CAPI call
  duration) in the existing allowlist paragraph. Publishes with the
  feature, not before.
- Release notes: A ships as its own line ("EDDA now knows your
  carrier..."), the website blurb stays one paragraph.

## 7. Open rulings for the maintainer

1. Approve the A/B/C split and order (A now).
2. B architecture: client-side only (recommended).
3. ~~Redirect~~ MEASURED: direct `edda://auth` passes; trampoline unbuilt (§4.2).
4. C: confirm it rides the data-source choice (local planner + server
   endpoint, same crate function) rather than needing its own ruling.
5. Finding a carrier-owning tester on Discord for B's `200` path.
