# EDDA API service design

Status: initial design, 2026-08-29

## Goals

The service supplies EDDA clients with community galaxy baselines without making every desktop installation download and ingest the upstream Spansh dumps. Every EDDA client continues to maintain its own EDDN connection for live local updates. The service must:

- hydrate from a reproducible initial source (synthetic fixtures first; approved bulk sources later);
- consume EDDN continuously to keep the next published baseline current, without replacing client EDDN;
- preserve source timestamps and the existing stale-update rules;
- publish small inhabited-system/catalog snapshots and large routing indexes safely;
- answer freshness-sensitive market, outfitting, shipyard, station, system and Powerplay queries;
- rebuild and publish routing/index artifacts on a schedule;
- run on a single Linux host initially, including under WSL for development;
- recover without requiring clients to understand the server's database layout.

The service is not initially an account service. It accepts no commander journals, credentials, or client writes.

## Existing code to reuse

| Concern | Existing owner | Server use |
|---|---|---|
| EDDN socket, reconnect/backoff and frame decoding | `ed-eddn` | Reuse directly with its `live` feature. |
| EDDN merge and freshness semantics | `ed-store::eddn` | Extract shared freshness/merge rules. Keep the SQLite adapter for clients and add a PostgreSQL adapter for the service. |
| Initial populated/station dump parsing | `ed-store::galaxy` | Done: `ed_store::galaxy::spansh` streams any Spansh dump into a `GalaxySink`; the SQLite importer and `ed-api`'s PostgreSQL hydration (`hydrate --spansh`) are the two adapters. Synthetic JSON fixtures remain a separate, tiny source. |
| Relational schema and lookups | `ed-store` | Reuse domain types and query semantics. Clients retain SQLite; the service uses PostgreSQL. |
| Compact memory-mapped routing format and importer | `ed-galaxy` | Done for on-demand builds: `ed-api build-routing` imports, validates (`Galaxy::validate_dir`), hashes and publishes the `routing` product atomically. Scheduling is still manual. |
| Route, market and nearest-service query logic | `ed-route`, `ed-store::lookup` | Reuse in HTTP handlers; do not duplicate SQL in the API crate. |

The Tauri application remains a client. Server orchestration must not be placed in `src-tauri`.

## Proposed workspace crates

### `ed-api`

A Linux-friendly binary using Tokio, Axum, SQLx and PostgreSQL. It owns configuration, HTTP routing, process lifecycle, health/readiness, metrics, its EDDN consumer task, database pools and scheduled jobs.

### `ed-sync`

A small library shared by the server and desktop client. It owns versioned manifest types, artifact metadata, SHA-256 verification, compatible schema versions, download/resume rules and atomic client-side activation. It contains no HTTP server or Tauri code.

The first implementation can start with `ed-api` and move manifest types into `ed-sync` as soon as the desktop consumes them. Avoid a speculative framework of many service crates.

## Runtime architecture

```text
Spansh/synthetic seed ----> hydration job -----------+
                                                      |
Server EDDN -> decoder -> bounded queue -> batched writer -> PostgreSQL
                                             |        |
                                             |        +-> read-only HTTP queries
                                             |
                                             +-> counters / freshness watermark

PostgreSQL -> consistent export -> SQLite snapshot -> staging -> validate/hash -> publish
all-star source -> ed-galaxy build -> staging dir -> validate/hash -> atomic publish

HTTP /manifest -> immutable artifacts -> EDDA client download/verify/merge/activate

Each client: API baseline -> local SQLite -> independent continuous EDDN updates
```

PostgreSQL is the service's canonical store. Server EDDN messages enter a bounded channel and are applied in batches (for example, up to 500 messages or 250 ms per transaction). Backpressure is visible in metrics; messages are never silently discarded. HTTP queries use a separate connection pool and do not queue behind ingestion.

PostgreSQL is preferred on the service because it supports continuous writes, concurrent public reads, background exports, migrations, operational tooling and future replicas. PostGIS is optional; ordinary coordinate bounding boxes are sufficient initially. TimescaleDB is not required.

The desktop remains SQLite-based. Shared code exposes domain inputs, outputs and freshness decisions, while thin PostgreSQL and SQLite repository adapters implement database-specific SQL. Contract tests run identical EDDN sequences against both adapters.

Bulk hydration and index publication do not mutate files being served. Every job writes a staging version, validates it, calculates hashes and sizes, then atomically replaces one small manifest pointer. Old versions remain available for a retention window so an interrupted client can resume.

## Data products

Different data has different freshness and size characteristics and should not be forced into one download.

### Bootstrap catalog

An immutable compact SQLite snapshot containing system address, name, coordinates, population and eventually permit metadata. The current 128,719 populated-system prototype is 14 MB uncompressed. This is suitable for bundling with the installer or downloading on first launch and supports nearby-system selection and basic offline lookup.

### Community data snapshot

A compact client database containing systems, stations and selected service metadata. Whether market/outfitting/shipyard rows belong in this artifact will be decided from measured size and churn. It is published periodically, not copied from the live database file.

### Community baseline and optional query API

The client baseline should include the useful current systems, stations, market, outfitting and shipyard rows after size/churn measurement and compaction. Clients then keep those rows fresh through their own EDDN listener. The server may expose equivalent query endpoints for diagnostics, web consumers or fallback, but the desktop does not depend on the API for live queries. We must not ship the present unfiltered 32 GB database; the export is purpose-built and normalized. Every row retains observation timestamp and provenance.

### Routing index

Versioned `ed-galaxy` artifacts published independently from community data. The manifest describes a logical index version and its files/shards. Large files support HTTP range requests and resumable download. Later, spatially stable shards can avoid redownloading an entire galaxy build when only metadata changes.

## HTTP API v1

Initial endpoints:

- `GET /healthz`: process alive; no dependency checks.
- `GET /readyz`: PostgreSQL readable, hydration complete, the server EDDN task state known, and a valid manifest published.
- `GET /v1/manifest`: current compatible artifacts, versions, sizes, SHA-256 hashes, creation times, minimum client/schema versions and source watermarks.
- `GET /v1/artifacts/{product}/{version}/{file}`: immutable file; `ETag`, `Last-Modified`, `Accept-Ranges`, and long-lived cache headers.
- `GET /v1/systems/near`: nearby systems from coordinates or an origin name, with population/permit filters and a maximum distance.
- `GET /v1/systems/{name}`: exact system lookup.
- `GET /v1/stations/search`: existing station/service search semantics.
- `GET /v1/market/search`, `/v1/outfitting/search`, `/v1/shipyard/search`: existing lookup semantics with explicit freshness fields.
- `GET /v1/status/data`: row counts, latest EDDN timestamps and published artifact versions; safe operational detail only.

Responses use a versioned envelope for errors and metadata. Artifact files are the sync protocol; the service exports language-neutral EBEX (EDDA Binary Exchange Format) baselines and never exposes live PostgreSQL storage or arbitrary database pages. Clients hydrate EBEX into SQLite and then continue applying their own EDDN stream. The normative byte layout, compatibility rules, and producer contract are in [BINARY-FORMATS.md](BINARY-FORMATS.md).

### Compatibility policy

HTTP and artifact compatibility are versioned independently. HTTP paths carry a major version (`/v1`); adding optional response fields is compatible, while removing or changing field meaning requires a new major path. Manifests carry a protocol version, and each product carries its own schema version plus an optional minimum client version. Clients advertise or locally know the protocol and product-schema versions they support, ignore unknown optional fields and products, and retain their active baseline when no compatible publication exists. Published artifact URLs are immutable. The service retains current and prior manifests and artifacts long enough for older supported clients to complete resumable downloads; server deployment does not rewrite an existing version in place.

## Manifest sketch

```json
{
  "protocol": 1,
  "generated_at": "2026-08-30T07:00:00Z",
  "eddn_watermark": "2026-08-30T06:59:58Z",
  "products": {
    "bootstrap": {
      "version": "2026-08-30.1",
      "schema": 1,
      "files": [{"path": "community-2026-08-30.1.ebex.zst", "bytes": 5000000, "sha256": "..."}]
    },
    "routing": {
      "version": "2026-08-30.1",
      "schema": 2,
      "files": [{"path": "stars.bin", "bytes": 0, "sha256": "..."}]
    }
  }
}
```

Clients download to a staging directory and verify every hash and schema. Before activation, the client preserves local EDDN observations newer than the product's `eddn_watermark`, installs the baseline, then reapplies those observations through the normal `ed-store::eddn` merge path. Only after that succeeds does it switch the local manifest atomically. A failed update leaves the previous version usable.

## Client baseline and EDDN merge

The API is a bootstrap and synchronization source, not the live-data authority for a running client:

1. A fresh installation downloads the newest compatible baseline.
2. The client starts or continues its own EDDN listener.
3. Every locally received EDDN envelope is applied to SQLite and retained in a bounded replay journal until it is older than the active baseline watermark.
4. On a later baseline update, the client stages the snapshot without stopping EDDN, records a cutover point, replays all observations newer than the snapshot watermark into the staged database, briefly pauses the local writer, applies the final tail, and atomically swaps databases.
5. The client resumes writes against the new database and prunes replay entries covered by the active watermark.

The replay journal stores decoded normalized envelopes (or a stable internal operation form), not raw ZeroMQ frames. It is bounded by age and size. Freshness checks remain the final protection: replaying an envelope twice is safe, and neither an older server row nor an older EDDN message may replace a newer observation.

If no API is reachable, the client continues indefinitely with its current baseline and EDDN updates. If EDDN is unavailable, the baseline remains usable with visible age metadata. Neither dependency is required for the app to start.

## Hydration and scheduled jobs

Hydration is an explicit PostgreSQL import job with a recorded source identity, timestamp, byte count and result. For development it consumes checked-in synthetic fixtures that cover duplicate/stale EDDN updates, removed commodities, provisional systems, stations and Powerplay changes. Production bulk inputs remain configurable and disabled until their use is agreed with the source provider.

Both sources exist today: `ed-api hydrate <fixture.json>` and `ed-api hydrate --spansh <dump.json[.gz]>` (see `crates/ed-api/README.md`). Every board a bulk source supplies goes through the same `ed_store::postgres` write path as the EDDN feed, so the strictly-newer rule is applied once, in SQL, for every source, and re-hydration can never regress a fresher live observation.

Suggested initial schedule:

- EDDN: continuous, reconnecting with bounded exponential backoff.
- Bootstrap/community snapshot: daily after a WAL checkpoint, or on demand.
- Routing index: daily when a new source is available; do not rebuild merely because the clock fired if the source watermark is unchanged.
- Integrity check: after every build and weekly against PostgreSQL.
- Retention: current plus at least two prior successful artifact versions.

Only one heavy job runs at a time. Jobs use a filesystem lock plus a database job record so a restart can distinguish pending, running, failed and published builds.

## Deployment

The first target is one Linux application process, PostgreSQL and a reverse proxy:

- systemd service (the same binary runs directly in WSL during development);
- PostgreSQL managed locally or externally, with migrations completed before readiness;
- staging and published-artifact directories on the same filesystem for atomic rename;
- TLS and optional CDN/proxy caching in front of immutable artifacts;
- configuration through a file and environment overrides, with no secrets required for public EDDN/read-only operation;
- structured JSON logs, Prometheus metrics and explicit readiness.

Important metrics include EDDN connected state, last frame time, decoded/applied/skipped/error counts by schema, queue depth, transaction duration, newest observation by product, HTTP latency/error counts, job duration/result, artifact size and active version.

Backups cover PostgreSQL (base backup plus WAL, or the managed-service equivalent), configuration and published manifests. Artifact files can be regenerated, but retaining the current published version makes recovery faster.

## Failure and consistency rules

- An older EDDN snapshot never overwrites newer stored data on either PostgreSQL or client SQLite; shared freshness rules remain authoritative.
- A malformed or unsupported EDDN message increments a labelled counter and is sampled in logs without uploader identity.
- A server EDDN outage makes future baselines progressively stale but does not interrupt client EDDN or existing artifacts; readiness exposes server lag separately.
- A failed hydration/build never changes the published manifest.
- A client never activates a partial or hash-mismatched product.
- API responses preserve source age rather than presenting community observations as live facts.
- No commander-specific data is uploaded in v1.

## Implementation milestones

### 1. Service skeleton and fixtures

Add `ed-api`, configuration, Axum health endpoints, structured logging, a synthetic hydration command and an integration-test fixture. Run it natively in WSL.

### 2. Canonical writer and EDDN

Add the bounded queue and batched PostgreSQL writer. Extract database-independent EDDN freshness/merge decisions, implement the SQLx repository, and run identical replay/contract tests against PostgreSQL and SQLite. Prove stale-message rejection and full-snapshot deletion semantics.

### 3. Binary bootstrap artifact

Implement the EBEX container and ratify section schemas with golden vectors, beginning with the benchmarked market v1 schema. Export deterministic little-endian sections from PostgreSQL, zstd-compress the complete artifact, validate it with an independent reader, and hydrate staged SQLite on the client before replaying local post-watermark EDDN operations. Keep the format implementation independent of SQLx/rusqlite row types so external producers can implement the documented wire contract.

Generate, validate and publish the complete inhabited-galaxy/community snapshot and `/v1/manifest`. Add HTTP range/ETag behavior and an end-to-end download/hash/hydration test.

### 4. Client sync

Add `ed-sync`, desktop background download, resume, hash verification, the local EDDN replay journal, watermark-aware merge/cutover, progress UI and fallback to the bundled/previous snapshot. Then remove the inhabited-system import from onboarding.

### 5. Fresh query endpoints

Implement PostgreSQL versions of useful `ed-store::lookup` semantics in typed API handlers. Add pagination, request limits, freshness/provenance fields and load tests. Treat these as optional services, not the desktop's normal live-data path.

### 6. Routing publication

Run the existing `ed-galaxy` builder as a scheduled job, validate by opening the produced index and running known routes, then publish it as a separate product. Add client-side staged activation.

### 7. Production hardening

Systemd unit, reverse-proxy configuration, metrics/alerts, backup/restore drill, artifact retention and documented upgrade/rollback procedures.

## Decisions to validate with measurements

- Compressed size and rebuild time of the bootstrap and station/service-only snapshots.
- Compressed size and daily churn of market/outfitting/shipyard baseline rows, plus an appropriate retention horizon for stale stations.
- EDDN arrival rate, batch size, queue capacity and PostgreSQL write latency on the target server.
- Required client replay-journal age/size given real snapshot publication and client update cadence.
- Routing artifact sharding boundaries and how often each shard actually changes.
- Acceptable EDDN lag before `/readyz` reports degraded rather than ready.
- Retention duration based on real client update cadence and bandwidth.
