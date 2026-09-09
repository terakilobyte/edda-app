# EDDA API

The API service is a Linux-native process that owns the community PostgreSQL
store, continuous EDDN ingestion, and immutable EBEX artifact publication.
The desktop application remains an independent client.

When serving, it connects to EDDN automatically. `ed-eddn` decodes and
normalizes each message into a backend-neutral operation; a bounded channel
feeds PostgreSQL batches of up to 500 operations or 250 ms. Database failures
retry the same batch with backoff, and a full queue pauses relay consumption
instead of dropping observations. The desktop SQLite adapter consumes the
same normalized operation types and contract semantics.

## Local development (macOS / Linux / WSL)

The repo root carries a `compose.yaml` (PostgreSQL 16 on `127.0.0.1:55432`)
and an `.env.example` with the three variables the service reads:
`DATABASE_URL`, `EDDA_API_BIND`, `EDDA_API_ARTIFACT_DIR`. No task runner is
needed; this is the whole sequence from an empty machine to a served
manifest:

```sh
docker compose up -d postgres            # 1. database (or any PostgreSQL 16)
cp .env.example .env && set -a && . ./.env && set +a
cargo build --release -p ed-api          # 2. build once; the CLI is `target/release/ed-api`

# 3. migrate + hydrate. Migrations run on every connect, so the first
#    command that touches the database migrates it.
target/release/ed-api hydrate crates/ed-api/fixtures/synthetic-galaxy.json   # tiny seed
target/release/ed-api hydrate --spansh ~/dumps/galaxy_populated.json.gz      # real seed

# 4. publish the products
target/release/ed-api publish-community                       # community EBEX baseline
target/release/ed-api build-routing ~/dumps/galaxy.json.gz    # routing EDGX index

# 5. serve and check
target/release/ed-api serve &
curl -s http://127.0.0.1:8787/readyz
curl -s http://127.0.0.1:8787/v1/manifest | jq .
```

### Hydrating from Spansh dumps

`hydrate --spansh <file>` streams any of the Spansh dumps
(<https://spansh.co.uk/dumps>) into the service tables through the
`ed_store::galaxy` source/sink seam, with a PostgreSQL adapter:

| Dump | Use it for |
|------|------------|
| `galaxy_populated.json.gz` (~2 GB) | The service seed: inhabited systems, Powerplay, every station with its market, outfitting and shipyard boards. Start here. |
| `galaxy_stations.json.gz` | Stations outside the bubble (fleet carriers included). Optional, after `galaxy_populated`. |
| `galaxy.json.gz` (~100+ GB) | Every system: what `build-routing` indexes. Not needed for hydration. |

Compressed or uncompressed files both work. Each run is one recorded job in
`service_hydrations` (source identity, byte count, watermark, result).
Boards go through the same `ed_store::postgres` write path as the EDDN
feed, so a station-level strictly-newer rule applies: re-hydrating from an
older dump never regresses a fresher live observation, and re-running the
same dump applies nothing. Systems the feed only knew by name (provisional
negative addresses) are promoted to their real address, stations included.
Bodies and factions are decoded but have no service tables yet.

The command prints a JSON summary (`systems_applied`, `snapshots_applied`,
`snapshots_skipped`, `market_rows`, `parse_errors`). Expect the populated
dump to take on the order of an hour on a laptop; `RUST_LOG=ed_api=info`
logs progress as compressed bytes consumed.

### Routing product

`build-routing <galaxy.json[.gz]> [artifact_dir]` runs `ed_galaxy::import`
into a staging directory under the artifact root, validates the index by
opening it (`Galaxy::validate_dir`), hashes `stars.bin`, `cells.bin`,
`names.bin` and `byname.bin`, renames the directory to
`routing/<version>/` and merges the product into `current.json` as
`routing` (schema = EDGX format version) next to `community`. The run is
recorded in `artifact_publications`. Building the full galaxy needs ~8 GB
of RAM; `galaxy_populated.json.gz` builds a bubble-only index in a minute
for testing. Publishing either product keeps the other.

### Tests

The parsing/sink seam and the routing publication are covered without a
database (`cargo test -p ed-api`, `cargo test -p ed-store --test
galaxy_sink`). The PostgreSQL round trips are ignored unless
`EDDA_API_TEST_DATABASE_URL` points at a database they may truncate:

```sh
docker exec edda-postgres createdb -U edda edda_test
EDDA_API_TEST_DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_test \
  cargo test -p ed-api --test postgres -- --ignored
```

The Homebrew WSL development instance uses port `55432`:

```sh
PG_BIN="$(brew --prefix postgresql@16)/bin"
"$PG_BIN/pg_ctl" -D "$(brew --prefix)/var/postgresql@16" \
  -l /tmp/edda-postgresql.log -o "-p 55432 -h 127.0.0.1" start

export DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_dev
export EDDA_API_TEST_DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_test
RUSTC_WRAPPER= cargo test -p ed-api --test postgres -- --ignored
```

The `edda_dev` and `edda_test` databases have already been created. Stop the
development cluster with:

```sh
"$(brew --prefix postgresql@16)/bin/pg_ctl" \
  -D "$(brew --prefix)/var/postgresql@16" stop
```

Optional variables:

- `EDDA_API_BIND` defaults to `127.0.0.1:8787`.
- `EDDA_API_ARTIFACT_DIR` defaults to `.data/api/artifacts`.
- `EDDA_API_EDDN_RELAY` defaults to the public EDDN relay.
- `EDDA_API_EDDN_QUEUE_CAPACITY` defaults to `10000`; a full queue applies
  backpressure rather than dropping observations.
- `RUST_LOG` controls structured JSON log filtering.

`publish-community` takes a repeatable-read PostgreSQL snapshot, emits and
independently validates systems, stations, commodity/module/ship catalogs,
market, outfitting, and shipyard EBEX sections, zstd-compresses and hashes the
artifact, then publishes the immutable file and `current.json` atomically.

`GET /healthz` only reports process liveness. `GET /readyz` also checks
PostgreSQL, a completed hydration, and a valid protocol-1 `current.json`
manifest in the artifact directory. `GET /v1/manifest` serves that pointer;
`GET /v1/artifacts/{path}` serves only manifest-listed files with ETag,
immutable caching, and single-range download support.

## Endpoints the API-only client depends on

The rule (maintainer, 2026-09-07): every `/v1/*` answer is complete on its
own; nothing depends on the client finishing the computation from a
local copy.

### `POST /v1/trade/search`

Two request shapes on one route:

- **Report (v2)** — a body carrying `ship` gets the whole profit
  report, computed here through `ed_route::profit::assemble` (the same
  pipeline the local finder runs), so round trips pair from the full
  leg set and rings, exclusions and phase timings come back too:

  ```json
  {"system": "Deciat",
   "ship": {"cargo_capacity": 720, "jump_range_ly": 30.5, "laden_range_ly": 22.1},
   "constraints": {"radius_ly": 60, "max_age_hours": 48, "max_stops": 3},
   "from_station_id": null, "limit": 20}
  ```

  `constraints` is `ed_route::profit::Constraints` with every field
  optional (defaults as the finder's); the server clamps radius to
  500 ly, stations to 2,500 nearest (measured:
  `docs/benches/trade-rows-pull-2026-09-07.csv`), age to 15 min–30 d,
  rings to 5 stops, limit to 100. The answer is the serialized
  `ProfitReport` plus `"provenance": "server"` and `"as_of"`.
- **Legacy** — no `ship`: the one-way legs answer 0.2.9 clients read.

Both share the one-at-a-time gate (429 when the line is full) and the
minute cache. 404 for an unknown system.

### `GET /v1/stations`

The wire half of the ship computer's station lookups. Exactly one of:

- `?system=<name>` — stations in a system, class rank then name
  (`include_carriers=false`, `include_minor=true`, `limit=50`).
- `?near=<system>&service=<key>&radius_ly=50` — nearest with a
  service, distance order. `service` is a friendly key
  (`material_trader`, `technology_broker`, `interstellar_factors`,
  `universal_cartographics`, `black_market`, … or `market` /
  `outfitting` / `shipyard`) or the raw journal key; an unknown one is
  a 400 listing the accepted keys. `min_pad=s|m|l` filters after the
  query. The system resolves through the routing index first, then
  Postgres; unknown is a 422 `{"error":"unknown_system"}`; an empty
  sphere is `[]`.
- `?name=<prefix>` — stations whose name starts with the prefix (two
  characters or more), through the completion index.

Rows are `ed_store::lookup::StationInfo`'s shape plus `distance_ly`
(null except for `near`); `primary_economy`, `government` and
`controlling_faction` are null — the server does not hold them yet.
Limit is clamped to 100. Gated by the knowledge limiter.

## Compatibility

HTTP resources use a major version prefix such as `/v1`; additive response
fields do not require a new major version. Immutable artifact manifests have
an independent `protocol` version, and every product has its own `schema`
version plus an optional minimum client version. A client activates only
products whose protocol and schema it supports. Published artifacts are never
changed in place, so older clients can finish downloads against retained
manifests even after a newer version is published.
