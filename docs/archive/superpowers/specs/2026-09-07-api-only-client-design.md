# The API-only client

*Design, 2026-09-07. Maintainer-ruled in conversation with review; the assistant's
routing position (ledger, same day) folded in. Supersedes the
data-source-choice design of 2026-09-06.*

## The ruling

> The server API should just assume there is no local data, ever. That's
> what the ship computer is for, to also look in the player's journal
> and local db. Imagine if there was no local EDDN feed.

Every `/v1/*` answer is complete on its own. Nothing the server returns
may depend on the client finishing the computation from a local galaxy
or market copy. The ship computer is the only place local (the
commander's own journal-derived data) and remote (the API) are fused.

Two premises that were carried without examination, retired here:

- **"Works offline."** Elite is an online game. If the commander's
  network is down, the game is down, the journal is not being written,
  and there is nothing to guard or plot. The only real failure mode is
  *our API* being unreachable while the game is up; that is an
  availability requirement on the server (uptime, a load balancer, a
  second instance), not an argument for a data copy on every disk.
- **"Local is a source."** There is one source. Every client-side
  mechanism for reconciling two — `data_source`, `api_fallback`, the
  consent card, `coverage`, "a board nobody broadcast" — goes.

## Boundary

**Local, journal-derived, never leaves the machine:** ship and loadout,
fleet, fuel state, cargo and materials, missions, ranks, engineer
unlocks and blueprint access, carrier state, marks, the route being
followed, powerplay seen, combat stats, signal watch. The tools
`get_ship_status`, `list_ships`, `get_inventory`,
`material_shopping_list`, `missions`, `current_route`, `follow_route`
and their kin are untouched.

**API, the galaxy and other people's stations:** `plot_route` beyond the
bubble, `find_profit`, `market_search`, `station_market` for any station
the commander is not docked at, `find_system`, `find_station`,
`nearest_service`, `systems_near`, `stations_in_system`, the fuel-trap
guard's sphere. Each returns a complete answer; the client renders, it
does not finish.

**The one fusion that survives:** the docked station. The journal's
`Market.json` is the freshest board that exists for the station the
commander is sitting at. "I'm docked, what do I fill up with" is the
journal board joined with API destinations on this machine — a join on
the commander's own data, not on a copy of everyone else's.

**The bundled bubble stays.** `src-tauri/assets/bubble/*.bin.zst`
(2.9 MB compressed, 7.4 MB installed, populated systems only) already
ships in every build. It answers name completion and bubble-scale
plotting — trade-route following in particular, whose waypoints are
populated by definition — on the commander's machine at zero server
cost. It is a latency choice, not a fallback: a plot the bundle cannot
make (an unpopulated stepping-stone) goes to `/v1/route`.

**Gone from the client:** `data_source`, `api_fallback`,
`DataSourceChoice`, the consent card, `coverage`, `fallback:
"consented"`, the local EDDN client, `ed-store`'s market / ingest /
coverage / mining / materials, the galaxy download and sync UI,
`src-tauri/src/exchange.rs` (with its four `reopen_writer()` sites —
"one writer" finishes by deletion), and the 11–20 GB.

**Cut, said out loud:** `material_sources`. It needs bodies/ring data
the server does not ingest. It returns when the server does.

## Server

### `/v1/trade/search` becomes the whole profit finder

Today: top-100 legs by profit/t, one commodity per row, best-8 buy ×
best-8 sell per commodity. The client pairs round trips from that
truncated list and never finds one (first defect under the ruling;
`profit.rs:1036` documents the same trap on the local side).

After:

- **The ship is in the request**: cargo capacity, jump range, laden
  range, pad. cr/h is computed on the server and is what the ranking
  sorts on.
- **`ed-route::profit` runs on the server.** Postgres supplies candidate
  stations and fresh rows (the CTEs it already has); the Rust
  `best_legs` → `round_trips` → `rings` → `diversify` pipeline runs over
  that set, unchanged. One implementation, the same answers local gave.
- The response is the full `ProfitReport`: legs, round trips, rings,
  `stations_considered`, `excluded`, timings.
- **Measured before it ships:** server-side pairing + rings cost at
  Deciat (6,655 stations) and Wyrd (11,861), with the 253 ms query as
  the baseline. Pinned origins with the maintainer's ship are a test: same
  legs, round trips and rings as local `find_at`.

### Lookups that do not exist on the wire

Seven tools read the local `sys_*` tables with no remote half. They
become:

- `GET /v1/stations?system=<name>` — stations in a system, filtered
  like `stations_in_system_filtered`.
- `GET /v1/stations?near=<system>&service=<svc>&radius_ly=` — nearest
  with a service, like `nearest_with_service`.
- `GET /v1/stations?name=<prefix>` — `find_stations`.
- `/v1/knowledge/system`, `/v1/knowledge/sphere` — already cover
  `find_system` and `systems_near`.
- `/v1/market/station/{id}` — already covers `station_market`.

Every new endpoint ships with `edda_<name>_requests_total{outcome}` and
`edda_<name>_seconds`, per `crates/ed-api/src/metrics.rs`.

### Routing

In order, each measured before the next:

1. **`ed-api` calls `ed_galaxy::init_thread_pool`.** It never has: the
   desktop client reserves two cores and lowers priority; the server
   runs rayon's default 8-thread pool at normal priority, which is why
   two plots read 99.5 % CPU and trade search waits 15 s for a 253 ms
   query. Expectation: trade-behind-plots latency falls materially.
2. **Raise the plot gate** from `CONCURRENCY = 2` with the pool sized.
   Pre-registered: aggregate served plots within ±20 %, 503s at c=16
   from 75 % to near zero, plot p50 from ~245 ms to ~700 ms.
3. **Two lanes.** Interactive (bubble-scale, 30 s budget, most slots)
   and long (thorough / Colonia-scale, 120 s budget, one or two slots).
   A Beagle Point plot cannot eat the bubble plotters' capacity.
4. **Diagnose the 422s**: 1,749 `route_long` requests at c=50 came back
   unprocessable, not refused. A correctness question under the
   capacity one.

The product number becomes "N concurrent plots per box"; past it, the
answer is horizontal (load balancer, second ed-api, dedicated
Postgres), as the maintainer stated.

### Per-install keys

Every search is now a server call, and per-IP budgets punish strangers
behind one NAT. The client generates a random install ID on first run
and sends it as `X-EDDA-Install`; limiters key on it. It is a random
string that identifies an install to the limiter and nothing else —
no account, no identity, nothing that could locate a commander.
Budgets are re-derived from the first real call trace under the new
client; until then the 2,500/h placeholders stand.

## Migration

The constraint: **the client cannot delete a local path until the API
answer that replaces it has been measured complete.** Server first,
behind the existing client; each deletion gated on a number.

**Phase A — server grows, client untouched.** Nothing reaches users.

1. `init_thread_pool` in ed-api; measure. Deploy on the maintainer's word.
2. `/v1/trade/search` returns the full report. Pinned-route test
   against local `find_at`; pairing cost at Deciat and Wyrd.
3. `/v1/stations`. Parity with `lookup::` on pinned queries.
4. Routing gate A/B, lanes, the 422 diagnosis.
5. `X-EDDA-Install` accepted (ignored until the client sends it).

**Phase B — the client switches, one path at a time, each flown.**

1. Trade panel and `find_profit` read the server report; client-side
   pairing goes. Open on round trips when the top loop's cr/h beats
   the top leg's. *Flight: round trips appear on remote.*
2. Lookups to `/v1/stations` and the knowledge endpoints; trap guard
   to `/v1/knowledge/sphere` (bundle first when the target is in it).
   *Flight: a low-tank jump past the bubble edge gets the callout.*
3. `data_source` / `api_fallback` / `DataSourceChoice` / consent /
   coverage deleted. Trade-route following plots against the bundle,
   falls through to `/v1/route` on a miss. *Bench first:* top-N trade
   legs from several origins, the maintainer's ship and a 20 ly hauler,
   plotted against the bundle; expectation under 2 % misses, all at the
   rim. More than that and the bundle becomes a radius, not a
   population filter.
4. The big cut: local EDDN client, `ed-store` market side, download and
   sync UI, `exchange.rs`.

**Phase C — one release.** Users go from "choose a source" to "there is
no choice" in a single update. On first run the new version deletes its
galaxy/market directories and reports how much it freed; the release
notes say so up front, because it is 11–20 GB going away unasked.

**The flight that gates the release:** a full trade loop end to end on
the API-only build — plot, dock, board from journal, `find_profit` from
the docked station, round trips visible, a jump past the bubble edge
with the trap firing — with that session's server-side metrics captured
as the first real call trace. The per-install budgets are derived from
that trace.

## Deferred

`material_sources` (server-side bodies ingest); per-install budgets
until the trace exists; horizontal scaling until the number says so.
