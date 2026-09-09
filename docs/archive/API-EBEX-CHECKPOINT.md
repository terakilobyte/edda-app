# API and EBEX checkpoint

Updated 2026-08-29. This is the clean-session handoff for the Linux API,
pluggable EDDN storage, and EBEX server work. Read `docs/BINARY-FORMATS.md`
for the normative wire layout and `docs/API-SERVICE-DESIGN.md` for the service
architecture.

## Canonical decisions

- The exchange format is **EBEX** (EDDA Binary Exchange Format), not EDEX or
  EDSN. Magic is `EBEX\0\0\0\0`; artifacts end in `.ebex.zst`.
- HTTP/gRPC is not the bulk synchronization boundary. Versioned HTTP manifests
  point to immutable EBEX artifacts; clients hydrate SQLite locally.
- EDDN follows `message -> normalized domain operation -> storage backend`.
  `ed-eddn` handles transport/decoding/normalization. `ed-store` owns both the
  SQLite and PostgreSQL adapters, which share freshness behavior.
- Systems and stations use freshness-guarded upserts. Market, outfitting, and
  shipyard messages are complete station snapshots: apply newer memberships,
  delete older missing memberships, and reject stale snapshots.
- Catalog rows persist independently of current membership. For example, a
  commodity can remain in the catalog after a newer market snapshot removes it
  from a station.
- Client baseline hydration should build a staged SQLite database and atomically
  activate it, rather than updating millions of rows in place. Newer local EDDN
  operations must be replayed before activation.

## Implemented architecture

- `crates/ed-domain`: backend-neutral normalized operations and apply stats.
- `crates/ed-eddn`: EDDN connection, decoding, validation, normalization.
- `crates/ed-store/src/sqlite.rs`: SQLite operation adapter.
- `crates/ed-store/src/postgres.rs`: PostgreSQL operation adapter.
- `crates/ed-sync`: EBEX container, codecs, validation, compression, inspector,
  golden vectors, bounds checks, checksum checks, required-section policy.
- `crates/ed-api`: PostgreSQL service, migrations, hydration, EDDN ingestion,
  EBEX publication, manifest/artifact HTTP endpoints.

Compatibility facades remain at `ed_store::eddn` and `ed_api::eddn` while
callers migrate to the new ownership boundary.

## EBEX sections currently published

> Erratum: only the markets section is published with the required bit set —
> see "Required-section policy" in `BINARY-FORMATS.md`. The community
> artifact also gained a ninth (stars, section 16) product since this
> checkpoint. The list below is otherwise accurate as of its date.

The community artifact contains eight schema-v1 sections built from
one repeatable-read PostgreSQL transaction:

| ID | Section | Record layout |
|---:|---|---|
| 1 | systems | 80-byte system identity/freshness records |
| 2 | stations | 48-byte station identity/service/freshness records |
| 4 | commodities | 16-byte catalog records plus string table |
| 5 | markets | 34-byte station/commodity rows plus snapshot directory |
| 6 | modules | 8-byte catalog records plus string table |
| 7 | outfitting | 16-byte station/module rows plus snapshot directory |
| 8 | ships | 8-byte catalog records plus string table |
| 9 | shipyards | 16-byte station/ship rows plus snapshot directory |

Catalog IDs are deterministic and artifact-local. PostgreSQL collation is not
trusted: symbols and relation rows are sorted in Rust by canonical catalog IDs,
which keeps output stable across database locales. The market embedded
commodity dictionary is retained for transitional compatibility and validated
against section 4.

Outfitting and shipyard auxiliary directories include stations with a known
complete empty snapshot, allowing clients to delete stale memberships safely.
Their timestamps must match the corresponding station-section freshness value.

The schemas are implemented internal candidates. Do not describe them as
externally stable until the readiness gates at the end of
`docs/BINARY-FORMATS.md` are complete.

## Latest validated publication

Generated from the Homebrew PostgreSQL development database:

```text
artifact: community/7/community-7.ebex.zst
compressed bytes: 89,584
uncompressed bytes: 502,352
sha256: ae061d7cf94440942a39b42f0014ee3d2b849229554fb0051dd7bdb41a619720
systems: 233
stations: 44
commodities: 383
market rows: 6,698
market station snapshots: 39
modules: 1,093
outfitting rows: 10,182
outfitting station snapshots: 16
ships: 48
shipyard rows: 375
shipyard station snapshots: 14
```

The artifact and current manifest are under `.data/api/artifacts/` and are
gitignored development output.

## Validation completed

These passed after the catalog-collation correction:

```sh
RUSTC_WRAPPER= cargo test -p ed-sync -p ed-api --lib
RUSTC_WRAPPER= cargo clippy -p ed-sync -p ed-api --all-targets --no-deps -- -D warnings
EDDA_API_TEST_DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_test \
  RUSTC_WRAPPER= cargo test -p ed-api --test postgres -- --ignored --nocapture
RUSTC_WRAPPER= cargo run -p ed-sync --example inspect -- \
  .data/api/artifacts/community/7/community-7.ebex.zst
```

The PostgreSQL integration test also applies the same normalized operation
sequence to in-memory SQLite and PostgreSQL, then compares stale-message,
market, outfitting, shipyard, and system results.

The complete Tauri test target is not currently a useful WSL gate because the
desktop crate needs native D-Bus/GTK development libraries. Do not install a
large GUI stack merely to validate server-only work.

## Local PostgreSQL

Homebrew PostgreSQL 16 runs on `127.0.0.1:55432` with existing databases
`edda_dev` and `edda_test`:

```sh
PG_BIN="$(brew --prefix postgresql@16)/bin"
"$PG_BIN/pg_ctl" -D "$(brew --prefix)/var/postgresql@16" \
  -l /tmp/edda-postgresql.log -o "-p 55432 -h 127.0.0.1" start

export DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_dev
export EDDA_API_TEST_DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_test
```

Publish and inspect with:

```sh
DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_dev \
EDDA_API_ARTIFACT_DIR=.data/api/artifacts \
RUSTC_WRAPPER= cargo run -p ed-api -- publish-community

RUSTC_WRAPPER= cargo run -p ed-sync --example inspect -- \
  .data/api/artifacts/community/<sequence>/community-<sequence>.ebex.zst
```

## Exact next task

Implement staged SQLite hydration for EBEX sections 4, 6, 7, 8, and 9, then
integrate them with the existing systems/stations/market hydration path.

1. Decode and validate every required section before mutating staging state.
2. Insert commodity, module, and ship catalogs using artifact-local IDs only as
   decode-time references; map symbols into SQLite's own identities.
3. Apply outfitting and shipyard station snapshots with the shared
   newest-observation-wins and delete-missing rules.
4. Preserve/replay local EDDN observations newer than each EBEX snapshot.
5. Add empty-snapshot, stale-local-row, newer-local-row, bad-reference, and
   rollback tests.
6. Activate the fully hydrated database atomically and retain the previous
   known-good baseline until startup verification succeeds.

Likely client entry point: `src-tauri/src/exchange.rs`. Keep EBEX codec/domain
types independent of rusqlite and SQLx row types.

## Workspace caution

The working tree contains extensive concurrent Windows-agent changes and the
new API/domain/sync crates are currently untracked as directories. Do not use
`git reset`, mass formatting, or broad cleanup. Inspect `git status` first and
edit only task-relevant files.
