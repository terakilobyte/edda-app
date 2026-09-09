# Checkpoint: EBEX, EDGX, API/client sync

Date: 2026-08-29
Repository: (the working tree)
Last committed revision: `d27e640 Improve ingestion and guided setup`

## Working-tree warning

The working tree is intentionally uncommitted and contains intertwined work
from this Windows/client session and a concurrent WSL/API session. Do not stage
or commit everything blindly. In particular, the new `ed-api`, `ed-domain`,
`ed-sync`, PostgreSQL storage, and much of the `ed-store`/`ed-eddn` refactor
originated in the WSL server session.

No commit was created after `d27e640`.

## Settled architecture

- The format name is **EBEX: EDDA Binary Exchange Format**.
- Uncompressed magic is the eight bytes `45 42 45 58 00 00 00 00`, commonly
  written `EBEX\0\0\0\0`.
- Artifact extension is `.ebex.zst`.
- JSON is used only for the mutable manifest. Actual versioned bulk payloads
  exchanged by the EDDA API and clients use EBEX.
- Published artifact URLs are immutable, but unreferenced artifacts need only
  be retained for 24 hours. The manifest must never point at a deleted file.
- Clients continue to maintain their own EDDN subscriptions. Server snapshots
  are baselines; newer local EDDN observations win.
- PostgreSQL is the canonical API/server store. SQLite remains the client store.
- EDGX remains the extracted, memory-mapped routing-index layout, transported
  as an EBEX product when downloaded.

## Implemented in this checkpoint

### EBEX protocol and fixtures

- Reconciled remaining `EDEX` names to `EBEX` across shared protocol, API,
  client, documentation, filenames, errors, temporary SQLite tables, and
  metadata keys. A case-insensitive search over the relevant repository areas
  found no remaining `EDEX` references.
- `crates/ed-sync` now has shared required-section policy and complete market-v1
  semantic validation for ordering, dictionary references, station-snapshot
  references, and timestamp agreement.
- Added exact market-only fixture:
  - `crates/ed-sync/fixtures/ebex-v1-market.hex`
  - `crates/ed-sync/fixtures/ebex-v1-market.json`
  - 216 bytes, SHA-256
    `c1e5324ce6caa709b4d82e1f4f015277f2297d3dd9e7efb6211fbcaa42b6623f`
- Added an exact eight-section fixture covering systems, stations, commodities,
  markets, modules, outfitting, ships, and shipyards:
  - `crates/ed-sync/fixtures/ebex-v1-full.hex`
  - `crates/ed-sync/fixtures/ebex-v1-full.json`
  - 1,128 bytes, SHA-256
    `c2703803a865f90a2bc67d3864a811b29db93ffd9ef46ea060ea98ae641b22a0`
- Added reproducible fixture generators:
  - `crates/ed-sync/examples/golden.rs`
  - `crates/ed-sync/examples/full_golden.rs`
- Added malformed-input coverage for every truncation, arithmetic overflow,
  checksum corruption, bad UTF-8, reserved fields/bits, incorrect record sizes,
  unknown optional/required sections, container versions, and all current
  required section-schema versions.

### Independent implementation

`tools/verify-binary-fixtures.mjs` is a dependency-free Node implementation
that independently parses and verifies:

- the market-only EBEX fixture;
- all eight sections in the full EBEX fixture;
- every EDGX companion file;
- CRC-32C, SHA-256, offsets, records, string tables, station snapshots, spatial
  cells, and by-name ordering.

Run it with:

```text
node tools/verify-binary-fixtures.mjs
```

Expected output:

```text
verified EBEX goldens (216 + 1128 bytes) and EDGX golden (191 bytes)
```

### EDGX specification and validation

`docs/BINARY-FORMATS.md` now normatively defines:

- the complete `cells.bin` table, cell-coordinate calculation, 21-bit packing,
  ordering, coverage, and lookup behavior;
- the complete `byname.bin` table, permutation, Unicode lowercase collation,
  tie-breaking, and autocomplete behavior;
- every star-class nibble code from 0 through 15, including scoopability,
  supercharge multiplier, source-class grouping, and hazards.

Added EDGX fixture files under `crates/ed-galaxy/fixtures/edgx-v2/`, decoded
expectations/digests, and `crates/ed-galaxy/examples/golden.rs`.

`Galaxy::validate` / `Galaxy::validate_dir` performs the full linear-time
publication scan without slowing normal client startup. It validates exact file
sizes, reserved bytes, finite coordinates, UTF-8/name bounds, cell coverage and
packing, and the complete by-name permutation/order.

### API publication edge case

`ed-api` semantic validation now accepts a valid empty market snapshot without
requiring a separate commodity section. A separate commodity catalog is checked
when present; the transitional embedded market dictionary remains valid.

## Verification completed

- `cargo test -p ed-sync`: 14 passed.
- `cargo test -p ed-galaxy --lib`: 18 passed, one release benchmark ignored.
- `cargo test -p ed-api --lib`: 9 passed.
- `cargo test -p edda --lib`: 31 passed.
- `node tools/verify-binary-fixtures.mjs`: passed.
- Relevant case-insensitive search found no remaining `EDEX` or `.edex` text.

## Remaining publication gate

The SQLite/PostgreSQL parity contract already exists in
`crates/ed-api/tests/postgres.rs`. It applies baseline, replacement/deletion,
stale, and newer operations for markets, outfitting, shipyards, and system data,
then compares normalized SQLite and PostgreSQL results.

It was not executed here because `EDDA_API_TEST_DATABASE_URL` was not set. Run
this only against a disposable database because the test truncates service
tables:

```text
EDDA_API_TEST_DATABASE_URL='postgres://...' \
  cargo test -p ed-api --test postgres -- --ignored --nocapture
```

Production artifact publication remains gated on a successful run. Update the
readiness table in `docs/BINARY-FORMATS.md` afterward.

## Important next integration issue

The server-side publisher has advanced to an eight-section EBEX baseline, while
the desktop hydrator in `src-tauri/src/exchange.rs` currently activates only
market data. It correctly rejects unsupported **required** sections. Before a
full eight-section manifest is offered to released clients, do one of the
following deliberately:

1. implement client hydration for systems, stations, commodities, modules,
   outfitting, ships, and shipyards; or
2. publish only market as required and mark unsupported sections optional until
   the matching client version is available.

Do not weaken required-section rejection merely to make the current client
accept the artifact.

## Other known context

- The full workspace test previously reproduced four `ed-route` profit-test
  failures after the shared storage refactor. Likely cause: SQLite
  `datetime(..., 'unixepoch')` produces `YYYY-MM-DD HH:MM:SS`, while the shared
  `ed_eddn::epoch_secs` parser expects journal/EDDN RFC3339. Recheck whether the
  WSL agent already corrected this before changing it.
- The public API endpoint does not yet exist. Client onboarding temporarily
  falls back to legacy direct Spansh import when no community API URL is set.
- The agreed unreferenced-artifact retention policy is 24 hours; never delete
  the artifact referenced by the active manifest.
- A repository-grounded public feature/documentation inventory was produced in
  the prior session but was not saved as a separate document. One confirmed
  documentation defect remains: Help describes three Galaxy Map capture points;
  guided setup now uses four (Search, First Result, Target, Plot Route).

## Files central to resuming

- `docs/BINARY-FORMATS.md`
- `docs/API-SERVICE-DESIGN.md`
- `crates/ed-sync/src/lib.rs`
- `crates/ed-sync/fixtures/`
- `crates/ed-galaxy/src/format.rs`
- `crates/ed-galaxy/src/import.rs`
- `crates/ed-galaxy/fixtures/edgx-v2/`
- `tools/verify-binary-fixtures.mjs`
- `crates/ed-api/src/snapshot.rs`
- `crates/ed-api/tests/postgres.rs`
- `src-tauri/src/exchange.rs`

## Safe restart instruction

Start by reading this checkpoint, `docs/BINARY-FORMATS.md`, and current
`git status --short`. Assume every uncommitted file may belong partly to the
concurrent WSL session. Inspect provenance before staging or committing.
