# EDDA as an EDDN sender — design (2026-09-30)

Boss rulings, 2026-09-30: EDDA uploads the commander's station boards to
EDDN; **opt-in**, exposed in Settings and in onboarding; **all three
boards** (outfitting/3, commodity/3, shipyard/2) from the start;
`uploaderID` is the **commander name** as EDDN's guidance says;
`softwareName` is **EDDA**.

## Why

EDDN's outfitting/2 boards are bare symbols, and a merc-coin
pre-engineered variant shares the plain module's symbol. Only
outfitting/3, with `BuyPrice` and `BuyMercCoinsPrice` per entry, tells
them apart. Measured 2026-09-30 (`docs/benches/2026-09-29-eddn-outfitting-v3-share.csv`):
10 of 98 boards on the relay in 600 s were v3, from EDO Materials Helper
and EDDI; EDMC (73) and EDDiscovery (7) are v2. Every EDDA commander who
opts in adds priced boards to the commons, and EDDA's own market board
gets fresher for everyone. No permission or registration is required by
EDDN; its rules are in `docs/Developers.md` and the schema READMEs, and
this design follows them to the letter.

## Sources (all already on disk)

| Need | Source |
|---|---|
| Outfitting board | `Outfitting.json` (`timestamp`, `MarketID`, `StationName`, `StarSystem`, `Horizons`, `Items[{id, Name, BuyPrice, BuyMercCoinsPrice?}]`) |
| Market board | `Market.json` (`Items[{id, Name, Name_Localised, Category, Category_Localised, BuyPrice, SellPrice, MeanPrice, StockBracket, DemandBracket, Stock, Demand, Consumer, Producer, Rare}]`) |
| Shipyard board | `Shipyard.json` (`AllowCobraMkIV`, `PriceList[{id, ShipType, ShipType_Localised, ShipPrice}]`) |
| Where we are | latest `Docked` event (`MarketID`, `StationName`, `StarSystem`) |
| Game version | current journal `Fileheader` (`gameversion`, `build`), e.g. `"4.4.1.1"`, `"r332841/r0 "` — passed through verbatim, whitespace included |
| Expansion flags | latest `LoadGame` (`Horizons`, `Odyssey`) |
| Commander | latest `LoadGame.Commander` (also `Commander` event) |
| App version | `CARGO_PKG_VERSION` |

The store already keeps the three files as snapshots (`snapshots` table,
`ingest.rs` `COMPANION_FILES`) and the watcher already reacts to
`snapshots_updated`.

## Architecture

Two units, one hook, one setting.

1. **`ed_eddn::upload` (pure).** Builds and validates each message from
   the file JSON plus a `Context { docked: Docked, fileheader: Fileheader,
   loadgame: LoadGame, commander: String, app_version: String, test: bool }`.
   Returns `Result<Envelope, Skip>` where `Skip` names the reason (no
   dock, station mismatch, beta build, empty board, not newer). No I/O.
   Fixture tests from the maintainer's real files (redacted only of the
   commander name).
2. **`edda::eddn_upload` (I/O).** Holds the last-sent `(market_id, kind,
   timestamp)` per kind in memory, the reqwest client, the setting.
   `on_snapshots(conn)` reads the three snapshots, the Docked/Fileheader/
   LoadGame context, calls the builder, posts, logs, counts.
3. **Hook.** In `watcher.rs`, in the existing
   `if stats.ingest.snapshots_updated > 0` block, after the outfitting
   board is recorded locally: `eddn_upload::on_snapshots(state, conn)`.
   Fire-and-forget on the async runtime so the ingest pass never waits on
   the network.
4. **Setting.** `Config.eddn_upload: Option<bool>` (None = off).
   Commands `eddn_upload_get` / `eddn_upload_set`, mirroring
   `telemetryPrefs`. Off until set true.

## Gate and consent text

- Settings → System: a switch "Share station data with EDDN" with the
  text: *"When you dock, EDDA sends that station's outfitting, market and
  shipyard lists to EDDN, the community data network that EDMC, Inara,
  EDSM and EDDA's own market search are built on. The message names the
  station and the goods, your commander name as its uploader (EDDN
  obfuscates it before anyone downstream sees it), your game version, and
  whether you have Horizons and Odyssey. Nothing about your ship, cargo,
  credits or position beyond the station you are docked at."*
- Onboarding: the same switch on the "Ready" step, beside the telemetry
  one, default off, with a link to EDDN's site.
- Never sends when off; flipping it on does not send anything until the
  next board change.

## Checks before every message (drop on any failure, log the reason)

1. Setting on.
2. `Fileheader.gameversion` starts with `4.` and neither it nor `build`
   contains `beta`/`alpha` (case-insensitive). EDDN: live schemas only.
   Legacy (3.8) never sends.
3. The file's `MarketID`, `StationName`, `StarSystem` equal the latest
   `Docked` event's; the latest location-changing event after that Docked
   is not an `Undocked`/`FSDJump`/`Location` elsewhere. EDDN: cross-check
   against prior location events; drop on mismatch.
4. The file `timestamp` is newer than the last sent for that
   `(market_id, kind)`. The game rewrites the files often; each board
   goes up once per visit unless it changes.
5. The list is non-empty after filtering (`minItems: 1`).
6. `timestamp` is RFC 3339 with `Z`.

## The three messages (per schema and README, checked 2026-09-30)

Common `message` keys: `systemName`, `stationName`, `marketId`,
`timestamp` from the file; `horizons` and `odyssey` from LoadGame,
**omitted** when unknown, never null.

- **outfitting/3** — `modules`: one object per `Items[]` entry:
  `{ "id": id, "Name": Name, "BuyPrice": BuyPrice, "BuyMercCoinsPrice": BuyMercCoinsPrice or 0 }`.
  Keep only names matching `(^Hpt_|^hpt_|^Int_|^int_|_Armour_|_armour_)`;
  drop `Int_PlanetApproachSuite` (README, historical). `uniqueItems`: the
  schema dedups by whole object, and the three medium seeker racks differ
  by id, so nothing is lost.
- **commodity/3** — `commodities`: one object per `Items[]` entry after
  dropping items whose `Category` is the non-marketable category and
  items with a non-empty `legality`; keys `name` (Name with `$` and
  `_name;` stripped), `buyPrice`, `sellPrice`, `meanPrice`, `stock`,
  `demand`, `stockBracket`, `demandBracket`, and `statusFlags` only when
  present. Drop `id`, `Category*`, `Producer`, `Consumer`, `Rare`,
  `StationType`, every `_Localised`. **No `economies`, no `prohibited`**
  (Market.json has neither; empty lists are forbidden).
- **shipyard/2** — `ships`: `PriceList[].ShipType` as given;
  `allowCobraMkIV` from the file.

`header`: `uploaderID` = commander name, `softwareName` = `"EDDA"`,
`softwareVersion` = app version, `gameversion` and `gamebuild` verbatim
from Fileheader. `$schemaRef` = `https://eddn.edcd.io/schemas/<kind>/<v>`
plus `/test` in debug builds, tests, and when `EDDA_EDDN_TEST=1`.

## Transport

`POST https://eddn.edcd.io:4430/upload/`, HTTP/1.1 only (reqwest with
`http1_only`), body gzip with `Content-Encoding: gzip`, 10 s timeout,
the app's User-Agent. Responses: 200 done; **400 and 426 never retried**,
logged at `warn` with the response body (it names the schema fault), 426
also sets a "EDDN schema outdated, update EDDA" notice in diagnostics and
disables further sends this session; 408/413/503 retried once after 60 s
then dropped. Any other error dropped with a `warn`. EDDN: no data is
better than bad data.

## Observability (ships measurable)

- One `info` line per attempt: `eddn upload` with `kind`, `market_id`,
  `station`, `items`, `status`, `ms`, `skip` reason when skipped.
- Counters in the diagnostics panel: sent / skipped-by-reason / failed,
  per kind, this session.
- Acceptance on the live schema after the boss's flight:
  `docs/benches/knobs/eddn_echo_check.py <seconds>` listens to the relay
  and prints every message whose `softwareName` is `EDDA`, so the first
  real upload is confirmed by the relay echoing it back, with its
  `gatewayTimestamp`.
- Bench record: `docs/benches/2026-09-30-eddn-upload-first-echo.csv`
  with the echo latency and the three kinds.

## Testing

- `ed_eddn::upload` unit tests: each kind from real fixture files;
  every check in the gate has a test that trips it; the commodity filter
  drops a non-marketable and a `legality` item; the outfitting filter
  drops `Int_PlanetApproachSuite`; `BuyMercCoinsPrice` defaults to 0;
  flags omitted when LoadGame is unknown; header verbatim including the
  trailing space in `build`.
- Schema validation in tests: the three live schemas vendored under
  `crates/ed-eddn/schemas/` and every built message validated with
  `jsonschema` in `cfg(test)` — a change to our builder that breaks the
  schema fails the suite before it reaches the gateway.
- `edda` tests: the sender never posts when the setting is off; a 400
  is not retried; a 503 is retried once.
- Manual: debug build against `/test`, watch the echo knob on the test
  schemas, then the boss flies, then one live send confirmed by the echo.

## Out of scope (YAGNI)

- CAPI as a source (EDDN discourages it; the journal files are fresher).
- Journal event schemas (FSDJump, Scan, …): a separate design.
- A retry queue across sessions: stale data must not be sent anyway.
- Uploading from the server.

## Ledger

`docs/ROADMAP.md` gets the ruling, the measured v3 share that motivated
it, and, after the flight, the first-echo numbers.
