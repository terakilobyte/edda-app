# EDDA binary interchange formats

Status: **EBEX container v1 plus systems, stations, commodities, market,
modules, outfitting, ships, shipyards, and stars schema v1 are the launch
formats. EDGX v2 is likewise fully specified and fixture-backed.** EDDA is
pre-release, so v1 is the first published version of every schema; there is
no earlier deployed version to stay compatible with. Production EBEX artifact
publication remains gated by the database-parity run recorded at the end of
this document.

This document is the language-neutral contract for EDDA's published binary
data. It is intended for EDDA clients and servers, independent tools, and data
providers such as Spansh. Rust structs are implementation details; bytes on
disk are the contract.

EDDA has two deliberately separate formats:

- **EDGX** is the immutable, memory-mapped galaxy routing index.
- **EBEX** (EDDA Binary Exchange Format) is the compressed exchange container
  used to hydrate a client's
  mutable SQLite database with inhabited-galaxy/community data.

Both formats use little-endian integers and IEEE-754 little-endian floats.
Neither serializes a native C/Rust struct or relies on compiler padding.

## Compatibility rules

The words MUST, MUST NOT, SHOULD, and MAY are normative.

- Readers MUST reject an unknown container major version.
- Readers MUST reject an unknown schema version for a required section.
- Readers MUST skip unknown optional section IDs using the directory offsets.
- Writers MUST set reserved bytes and unknown flag bits to zero.
- Readers MUST bounds-check every offset, length, count, and multiplication
  before allocating or slicing.
- UTF-8 fields MUST contain valid UTF-8 and are never NUL-terminated.
- Published files are immutable. A corrected artifact receives a new identity
  and URL; it is never rewritten in place.
- A client MUST verify the manifest SHA-256 before decoding or activating an
  artifact.
- Timestamps are signed Unix seconds in UTC. `0` means unknown, never “now.”
- Database freshness comparisons use the source observation timestamp, not
  download, import, or arrival time.

## EBEX: EDDA Binary Exchange Format

Filename conventions, by manifest product:

- `community` baseline: `community/<version>/community-<version>.ebex.zst`
- `stars` product: `stars/<version>/stars-<version>.ebex.zst`

The complete EBEX byte stream is compressed as one standard zstd frame. The
manifest records the compressed byte length and SHA-256. All offsets below
refer to the **uncompressed** EBEX stream. A producer SHOULD include the zstd
content size when known, but readers MUST also support streaming frames without
it.

The uncompressed layout is:

```text
64-byte header
N × 64-byte section directory entries
zero padding to an 8-byte boundary
section record and auxiliary regions
```

Alignment and padding are normative, not cosmetic: every region (directory,
each record region, each auxiliary region) starts on an 8-byte boundary, all
padding bytes MUST be zero, and the file MUST end exactly at the last
region's padded end. Validators reject nonzero padding and trailing bytes,
so two encoders producing the same logical content produce identical bytes.

### Header (64 bytes)

| Offset | Size | Type | Meaning |
|---:|---:|---|---|
| 0 | 8 | bytes | Magic: `45 42 45 58 00 00 00 00` (`EBEX\0\0\0\0`) |
| 8 | 2 | u16 | Container version; `1` |
| 10 | 2 | u16 | Header size; `64` |
| 12 | 4 | u32 | Flags; bit 0 = full baseline |
| 16 | 8 | u64 | Monotonic snapshot sequence |
| 24 | 8 | i64 | Artifact creation time |
| 32 | 8 | i64 | Inclusive source/EDDN watermark |
| 40 | 4 | u32 | Section count |
| 44 | 2 | u16 | Directory entry size; `64` |
| 46 | 2 | bytes | Reserved, zero |
| 48 | 8 | u64 | Directory offset; `64` in v1 |
| 56 | 8 | bytes | Reserved, zero |

Snapshot sequence is an artifact ordering key assigned by the publisher. It is
not an EDDN sequence and need not be globally meaningful outside that
publisher. Creation time is informational; the watermark governs replay and
freshness.

Normative header rules a validator enforces:

- The directory offset MUST be exactly `64` in container v1.
- Header flag bit 0 (full baseline) is currently always set by publishers;
  any other header flag bit MUST cause rejection.
- Header size MUST be `64` and directory entry size MUST be `64`.
- All reserved header bytes MUST be zero.

### Section directory entry (64 bytes)

Entries MUST be strictly ascending by section ID — sorted, with no
duplicates — and section regions MUST NOT overlap.

| Offset | Size | Type | Meaning |
|---:|---:|---|---|
| 0 | 2 | u16 | Section ID |
| 2 | 2 | u16 | Section schema version |
| 4 | 4 | u32 | Flags; bit 0 = required for baseline |
| 8 | 8 | u64 | Record count |
| 16 | 4 | u32 | Fixed record size, or `0` for variable records |
| 20 | 4 | bytes | Reserved, zero |
| 24 | 8 | u64 | Record-region offset |
| 32 | 8 | u64 | Record-region byte length |
| 40 | 8 | u64 | Auxiliary-region offset, or `0` |
| 48 | 8 | u64 | Auxiliary-region byte length, or `0` |
| 56 | 4 | u32 | CRC-32C of record then auxiliary bytes |
| 60 | 4 | bytes | Reserved, zero |

SHA-256 authenticates the downloaded artifact. CRC-32C is an inexpensive
post-decompression corruption check and does not replace SHA-256.

CRC-32C is the Castagnoli variant, fully specified so an independent
producer can implement it from this document alone: reflected polynomial
`0x82F63B78`, initial value `0xFFFFFFFF`, final XOR `0xFFFFFFFF`, computed
over the section's record bytes immediately followed by its auxiliary bytes
(without the padding between or after them).

Further normative entry rules a validator enforces:

- Directory flag bit 0 is `required for baseline`; any other bit MUST cause
  rejection.
- For fixed-size records, `record_count × record_size` MUST equal the
  record-region byte length.
- When the auxiliary byte length is `0`, the auxiliary offset MUST also be
  `0`; when nonzero, the auxiliary region MUST start at or after the padded
  end of the same section's record region.

### Section IDs

IDs are stable even while their record schemas evolve. Nine sections are
implemented and published; the rest of the table reserves IDs for planned
relations that have **no byte layout yet** — a v1 artifact never contains
them, and a reader encountering one treats it as an unknown section.

| ID | Name | Status | Purpose |
|---:|---|---|---|
| 1 | systems | implemented | Inhabited/provisional system identity, position, population and allegiance/power metadata |
| 2 | stations | implemented | Station identity, owning system, type, position, landing/economy metadata |
| 3 | station services | reserved | Station-to-service membership |
| 4 | commodities | implemented | Commodity dictionary and display metadata |
| 5 | markets | implemented | Per-station commodity prices, supply, demand and freshness |
| 6 | modules | implemented | Outfitting module dictionary and display metadata |
| 7 | outfitting | implemented | Station-to-module availability and freshness |
| 8 | ships | implemented | Ship dictionary and display metadata |
| 9 | shipyards | implemented | Station-to-ship availability and freshness |
| 10 | factions | reserved | Faction identity and current system/state metadata |
| 11 | bodies | reserved | Body identity and exploration/material metadata |
| 12 | body materials | reserved | Body-to-material abundance |
| 13 | rings | reserved | Ring identity and physical metadata |
| 14 | ring hotspots | reserved | Ring-to-hotspot material membership |
| 15 | prohibited markets | reserved | Station-to-prohibited-commodity membership |
| 16 | stars | implemented | Main-star class, scoopability and observation time per system |

Splitting logical relations into sections allows independent schema
evolution without changing the container version.

#### Required-section policy

The directory's `required for baseline` bit is a **compatibility gate**, not
a description of importance: a reader MUST reject an artifact whose required
section carries an `(id, schema)` pair it does not support, and MUST skip an
optional section it does not understand. That pairing is what lets a future
section version ship without breaking installed clients.

Publishers therefore mark exactly one community-baseline section required:
**markets (section 5)**. Released clients hydrate the market relation and
skip the rest; marking any other section required would brick every
installed client on its next sync. A section is promoted to required only
when no supported client would be broken by the promotion. The stars
product's single section is likewise published optional. The canonical
policy table lives in `ed-ebex` (`community_section_plans`,
`stars_section_plan`) and both publishers read it; the checked-in full
golden fixture pins the policy in bytes.

The installability check a market-only client runs before touching its
database is: container validation, then every *required* section's
`(id, schema)` is one it supports, then the full market-relation
validation (`validate_market_baseline` in the reference implementation).

### Systems schema v1 (section 1)

Systems are sorted by signed `address` and use an auxiliary string table.
String ID `0` means absent; `name_id` MUST be nonzero. Records are 80 bytes:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 8 | i64 | System address; negative values are provisional |
| 8 | 8 | f64 | X coordinate |
| 16 | 8 | f64 | Y coordinate |
| 24 | 8 | f64 | Z coordinate |
| 32 | 8 | u64 | Population |
| 40 | 8 | i64 | Latest source observation time |
| 48 | 4 | u32 | Name string ID |
| 52 | 4 | u32 | Security string ID |
| 56 | 4 | u32 | Allegiance string ID |
| 60 | 4 | u32 | Controlling-power string ID |
| 64 | 4 | u32 | Power-state string ID |
| 68 | 4 | u32 | Powers string ID |
| 72 | 4 | u32 | Flags: bit 0 coordinates present, bit 1 population present |
| 76 | 4 | bytes | Reserved, zero |

### Stations schema v1 (section 2)

Stations are sorted by unsigned market/station ID and use their own auxiliary
string table. Records are 48 bytes:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 8 | u64 | Station market ID |
| 8 | 8 | i64 | Owning system address |
| 16 | 4 | u32 | Name string ID, or `0` |
| 20 | 4 | u32 | Flags: market, outfitting, shipyard in bits 0–2 |
| 24 | 8 | i64 | Complete market observation time, or `0` |
| 32 | 8 | i64 | Complete outfitting observation time, or `0` |
| 40 | 8 | i64 | Complete shipyard observation time, or `0` |

Systems and stations v1 use this auxiliary string table:

```text
string_count: u32
repeat string_count times, sorted by id:
  id: u32
  byte_length: u32
  value: [u8; byte_length]
```

Writers assign IDs deterministically by sorting distinct UTF-8 values by raw
byte order. IDs are artifact-local and references MUST resolve within the same
section. Validators additionally enforce: string IDs are strictly ascending
(1-based, no duplicates), every value is non-empty (absence is expressed by
reference ID `0`, never by an empty table entry), and the table ends exactly
where its last value does — trailing bytes are rejected.

### Commodity catalog schema v1 (section 4)

Commodity records are sorted by canonical symbol. Catalog IDs are contiguous,
start at `1`, and remain artifact-local. Records are 16 bytes and use the
section's auxiliary string table:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 2 | u16 | Commodity catalog ID |
| 2 | 2 | bytes | Reserved, zero |
| 4 | 4 | u32 | Canonical symbol string ID |
| 8 | 4 | u32 | Display-name string ID, or `0` |
| 12 | 4 | u32 | Category string ID, or `0` |

Display names and categories come from the community-maintained FDevIDs
tables (github.com/EDCD/FDevIDs, `commodity.csv`), hydrated server-side via
`ed-api hydrate --fdev-ids`; a symbol not yet hydrated emits `0` here (and
zero-length name/category in the market dictionary), which readers treat as
absent. The tables are vendored in this tree: `crates/ed-journal/data/`
holds `commodity.csv` and `material.csv`, and `crates/ed-api/src/fdev_data.rs`
bakes the commodity `(symbol, display name, category)` table into the
server, regenerated from the upstream CSVs when Frontier adds commodities.
`hydrate --fdev-ids` applies an operator-supplied CSV when given a path and
the baked-in table otherwise. The FDevIDs repository carries no license
file; the data is reproduced with attribution (see `THIRD-PARTY-NOTICES.md`).

### Module and ship catalog schema v1 (sections 6 and 8)

Module and ship catalogs use the same 8-byte record layout, but references are
section-specific and MUST NOT cross catalogs. Records are sorted by canonical
symbol, and IDs are contiguous from `1`:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 4 | u32 | Module or ship catalog ID |
| 4 | 4 | u32 | Canonical symbol string ID |

Each catalog owns an independent auxiliary string table in the format defined
above.

### Outfitting and shipyard schema v1 (sections 7 and 9)

Outfitting and shipyard records are sorted by `(station_id, item_id)` and are
exactly 16 bytes:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 8 | u64 | Station market ID |
| 8 | 4 | u32 | Module ID for outfitting; ship ID for shipyard |
| 12 | 4 | bytes | Reserved, zero |

Their auxiliary region is a complete station snapshot directory:

```text
station_snapshot_count: u64
repeat station_snapshot_count times, sorted by station_id:
  station_id: u64
  observed_at: i64
```

The directory MUST include a station whose newest complete list is empty. A
reader applies a newer snapshot by upserting listed memberships and deleting
older memberships absent from that station's complete list. It MUST preserve
local memberships newer than `observed_at`.

### Market schema v1 (section 5)

Market v1 is the first implemented EBEX record schema and is the format used by
the Rust benchmark. Records are sorted by `(station_id, commodity_id)` and are
exactly 34 bytes:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 8 | u64 | Station market ID |
| 8 | 2 | u16 | Commodity dictionary ID |
| 10 | 4 | u32 | Buy price |
| 14 | 4 | u32 | Sell price |
| 18 | 4 | u32 | Demand |
| 22 | 4 | u32 | Supply |
| 26 | 8 | i64 | Observation time |

Prices, demand, and supply use `0` for unavailable/none, matching Elite journal
and EDDN market semantics. Commodity ID `0` is invalid. A market record MUST
reference an entry in the commodities section or the market section's embedded
commodity dictionary during the transitional implementation.

The market auxiliary region — this is the production layout the community
publisher emits and the client hydrator requires, not a benchmark
convenience — is:

```text
commodity_count: u16
repeat commodity_count times:
  id: u16
  symbol_length: u16
  name_length: u16
  category_length: u16
  symbol: [u8; symbol_length]
  name: [u8; name_length]
  category: [u8; category_length]
station_snapshot_count: u64
repeat station_snapshot_count times, sorted by station_id:
  station_id: u64
  observed_at: i64
```

The station snapshot directory is required even when a station currently lists
zero commodities. It proves when each complete market list was observed, which
lets a client remove absent stale rows without deleting newer local EDDN data.
Per-commodity timestamps alone cannot express that safely.

The production publisher emits symbols only in the embedded dictionary
(`name_length` and `category_length` are `0`); display names and categories
ride in section 4. Market v1 retains its embedded dictionary for transitional
reader compatibility; writers MUST keep the two definitions identical.

Normative market-relation rules the full validation enforces:

- Dictionary commodity IDs are strictly ascending and every symbol is
  non-empty.
- Station directory entries are strictly ascending by station ID.
- The auxiliary region ends exactly at the end of the station directory.
- Records are strictly ascending by `(station_id, commodity_id)` —
  duplicates are rejected, not deduplicated.
- Every record's station MUST appear in the station directory and every
  record's commodity MUST appear in the dictionary.
- Every record's `observed_at` MUST equal its station's directory
  `observed_at`: a station's rows are one atomic snapshot, not a mix of
  observations. The client hydrator re-enforces this on its side.

### Stars schema v1 (section 16)

The stars product carries the main star of each system the service has
learned: enough for the route planner to know highway (neutron / white
dwarf) and scoopable stars without a full galaxy download. It is published
as its own single-section artifact (`stars-<version>.ebex.zst`), a few
megabytes, refreshed independently of the community baseline.

Records are sorted **strictly ascending by signed address** — the same key,
same order, as the systems section; negative addresses are provisional
systems. One record per system. The section carries **no auxiliary region**
(a validator rejects one). Records are 24 bytes:

| Offset | Size | Type | Field |
|---:|---:|---|---|
| 0 | 8 | i64 | System address; negative values are provisional |
| 8 | 1 | u8 | Main-star class code (the EDGX star-class code table below) |
| 9 | 1 | u8 | Scoopable: `0` or `1`; any other value is rejected |
| 10 | 6 | bytes | Reserved; MUST be zero and readers MUST reject nonzero |
| 16 | 8 | i64 | Class observation time |

The class byte uses the same normative code table as EDGX star records (see
the star-class code table in the EDGX chapter); `0` (Unknown) is valid.
`scoopable` describes the main star itself, pre-resolved by the publisher —
a reader does not re-derive it from the class.

Merge semantics on the client are **journal-wins**: a star class the
commander observed in their own journal is never overwritten by a server
snapshot, and among server rows the newest `observed_at` wins. This is the
stars counterpart of the market rule that local data newer than the
watermark survives hydration.

### Freshness, deletion, and activation

An EBEX v1 full baseline is not an event log. Absence means absent as of the
watermark for relations with full-snapshot semantics (market, outfitting, and
shipyard lists). It does not mean a client should discard observations newer
than that watermark.

Two conflict rules follow from that and are enforced per row:

- **Per-station stale skip:** a station whose local board is newer than the
  artifact's snapshot for that station keeps its local board; the snapshot
  for that one station is skipped, not merged.
- **Journal-wins (stars):** a star class sourced from the commander's own
  journal is never overwritten by any server snapshot, regardless of
  timestamps.

A client activates a snapshot as follows:

1. Download to staging and verify compressed length and SHA-256.
2. Decode and validate header, directory, section CRCs, referential integrity,
   record ordering, and supported schema versions.
3. Hydrate a new SQLite database for a complete multi-section baseline. A
   section-only update MAY use one rollback-safe transaction against the active
   database.
4. Replay normalized local EDDN operations whose observation times are newer
   than the snapshot watermark. Shared stale-message rules remain authoritative.
5. Pause the local writer briefly, replay the final tail, and atomically swap
   databases/manifests.
6. Retain the previous known-good baseline until activation is confirmed.

The server follows the equivalent publish discipline: build in staging,
validate, rename atomically, then publish the manifest last.

### Producer requirements

An independent producer does not need EDDA source code. It MUST:

- emit canonical little-endian bytes rather than memory-dumping structs;
- use stable Elite identifiers and canonical symbols;
- sort each section according to its section schema;
- deduplicate primary keys using newest-observation-wins semantics;
- preserve the source observation timestamp on every freshness-sensitive row;
- choose a watermark no newer than every included source stream is complete;
- validate foreign-key references before publication;
- produce deterministic output for identical normalized input;
- publish byte length and SHA-256 alongside the `.ebex.zst` file.

Provenance and licensing remain manifest metadata, not binary record fields.
The manifest SHOULD identify the producer, upstream sources, source artifact
identity/checksum, and build software version.

## The sync protocol: manifest and resumable download

The bytes above travel under a small JSON protocol (reference implementation:
the `ed-sync` crate). It is part of the public contract: an independent
client needs it to find, verify, and resume artifacts.

### Manifest

Route: `GET /v1/manifest`. Artifacts are served under `/v1/artifacts/<path>`
where `<path>` is the `path` field of a manifest file entry, verbatim.

```json
{
  "protocol": 1,
  "generated_at": "2026-08-29T12:00:00Z",
  "eddn_watermark": "2026-08-29T11:59:00Z",
  "products": {
    "community": {
      "version": "43",
      "schema": 1,
      "minimum_client": null,
      "files": [
        { "path": "community/43/community-43.ebex.zst",
          "bytes": 498952536,
          "sha256": "…64 lowercase hex…" }
      ]
    },
    "stars": { "version": "44", "schema": 1, "minimum_client": null, "files": ["…"] },
    "routing": { "version": "8", "schema": 2, "minimum_client": null, "files": ["…"] }
  }
}
```

- `protocol` is the manifest protocol version; readers MUST reject an
  unknown value. Current value: `1`.
- Product keys are a closed vocabulary: `bootstrap`, `community`, `routing`,
  `stars`. Unknown keys are skipped, not errors.
- `schema` is the product's payload version — the EBEX section schema for
  `community`/`stars`, the EDGX version for `routing`. A client MUST skip a
  product whose schema it does not read.
- `minimum_client`, when present, names the oldest client version allowed to
  install the product; older clients skip it.
- `generated_at` and `eddn_watermark` are informational strings; the binary
  watermark inside the artifact governs replay.
- Every `path` MUST be a safe relative path: no absolute paths, no `..`
  segments, no backslashes. Clients MUST reject an unsafe path rather than
  resolve it.
- `bytes` and `sha256` describe the served (compressed) file. A client MUST
  verify both — exact length and SHA-256 over the downloaded bytes — before
  decompressing or activating anything.

### Resumable download

Artifacts are large and immutable, which makes resumption safe and simple:

- A client downloads to a staging file and records its own byte offset.
- On interruption it retries with `Range: bytes=<offset>-`. A `206 Partial
  Content` response appends at the offset; a `200 OK` response means the
  server ignored the range, and the client MUST restart the file from zero.
- A response whose total length disagrees with the manifest's `bytes` is an
  error, not something to reconcile.
- Retries are bounded with a fixed delay (the reference policy: up to 50
  retries, 2 s apart, for the streaming case); the count and delay are
  client policy, not protocol.
- Verification (length + SHA-256) always runs over the complete staged file
  after the last byte, never incrementally trusted from a previous session.

Multi-file products (`routing`) verify each file independently and install
with renames only after every file verifies, so an interrupted install never
leaves a mixed-version directory.

## EDGX: galaxy routing index v2

EDGX remains separate because clients memory-map it directly rather than
hydrate it into SQLite. One index directory contains four immutable files:

- `stars.bin`: header plus fixed star records sorted by spatial cell.
- `cells.bin`: sorted spatial cell ranges.
- `names.bin`: concatenated UTF-8 system names.
- `byname.bin`: record indices sorted by lowercase system name.

### `stars.bin` header (32 bytes)

| Offset | Size | Type | Meaning |
|---:|---:|---|---|
| 0 | 4 | bytes | Magic `EDGX` |
| 4 | 4 | u32 | Version `2` |
| 8 | 8 | u64 | Record count |
| 16 | 4 | f32 | Spatial cell edge in light years |
| 20 | 12 | bytes | Reserved/header metadata; zero unless specified by a later compatible revision |

### EDGX v2 star record (29 bytes)

| Offset | Size | Type | Meaning |
|---:|---:|---|---|
| 0 | 4 | f32 | X coordinate, ly |
| 4 | 4 | f32 | Y coordinate, ly |
| 8 | 4 | f32 | Z coordinate, ly |
| 12 | 1 | u8 | Low nibble star-class code; high nibble flags |
| 13 | 1 | u8 | Reserved metadata |
| 14 | 2 | u16 | UTF-8 name length |
| 16 | 8 | u64 | Elite system address/id64 |
| 24 | 5 | u40 | Little-endian offset into `names.bin` |

#### Star-class code table

The low nibble at star-record offset 12 is one of the following normative
EDGX codes. All 16 possible nibble values are assigned in version 2.

| Code | EDGX class | Included Elite/Spansh classes | Fuel-scoopable | FSD boost |
|---:|---|---|:---:|---:|
| 0 | Unknown | Missing or unrecognized class | No | 1.0x |
| 1 | O | O | Yes | 1.0x |
| 2 | B | B | Yes | 1.0x |
| 3 | A | A | Yes | 1.0x |
| 4 | F | F | Yes | 1.0x |
| 5 | G | G | Yes | 1.0x |
| 6 | K | K | Yes | 1.0x |
| 7 | M | M | Yes | 1.0x |
| 8 | L | L | No | 1.0x |
| 9 | T | T | No | 1.0x |
| 10 | Y | Y | No | 1.0x |
| 11 | Protostar | T Tauri (`TTS`), Herbig Ae/Be (`AeBe`) | No | 1.0x |
| 12 | Exotic | Wolf-Rayet, carbon, `MS`, and S-type stars | No | 1.0x |
| 13 | White dwarf | Elite journal classes beginning with `D`; Spansh White Dwarf subtypes | No | 1.5x |
| 14 | Neutron | `N`; Spansh Neutron Star | No | 4.0x |
| 15 | Black hole | `H`, `SupermassiveBlackHole`; Spansh Black Hole subtypes | No | 1.0x |

“Fuel-scoopable” describes the encoded main-star class itself. A record may
also be treated as refuel-capable when its `scoopable companion nearby` flag
is set. White dwarfs and neutron stars provide the listed supercharge for the
next jump; black holes, white dwarfs, and neutron stars are treated as arrival
hazards by EDDA route selection.

Readers MUST interpret codes according to this table rather than an
implementation enum. Producers MUST encode an unrecognized source class as
0. Changing an assigned code's meaning or adding another class requires a new
EDGX record version because no unassigned nibble values remain.

Current flag bits are bit 0 `main star` and bit 1 `scoopable companion within
1,500 ls`. Unknown bits must be ignored by compatible readers.

### `cells.bin` spatial index

`cells.bin` has no header. It is an array of 16-byte records, and its file
length must therefore be a multiple of 16. Only occupied cells are present.
All integer fields are unsigned and little-endian.

| Offset | Size | Type | Meaning |
|---:|---:|---|---|
| 0 | 8 | u64 | Packed spatial-cell key |
| 8 | 4 | u32 | Zero-based index of the first member in `stars.bin` |
| 12 | 4 | u32 | Number of consecutive star records in the cell |

For a position `(x, y, z)` and the positive, finite `cell_ly` value from the
`stars.bin` header, calculate signed cell coordinates as:

```text
cx = floor(x / cell_ly)
cy = floor(y / cell_ly)
cz = floor(z / cell_ly)
```

Each coordinate must be in `[-1,048,576, 1,048,575]`. Encode a coordinate as
the 21-bit unsigned value `p(v) = v + 1,048,576`, then pack the key as:

```text
cell_key = (p(cx) << 42) | (p(cy) << 21) | p(cz)
```

Records are strictly ascending by `cell_key`. `stars.bin` is sorted by that
same key, so every cell's members form the consecutive half-open range
`[start, start + count)`. Within a cell, star order has no semantic meaning.

A conforming index satisfies all of the following:

- `count` is nonzero and `start + count` does not exceed the star count.
- Cell ranges appear in ascending `start` order, do not overlap or leave gaps,
  and together cover every record in `stars.bin` exactly once.
- Recomputing the cell key from every referenced star position produces the
  key stored in its cell record.
- The empty galaxy is represented by an empty `cells.bin` file.

Readers locate a cell with binary search on `cell_key`, then scan only its
referenced slice of `stars.bin`. A radius query enumerates cells intersecting
the query volume and applies the exact distance test to their star records.

### `byname.bin` name index

`byname.bin` has no header. It contains exactly one 4-byte record for every
star record, so its required length is `star_count * 4` bytes.

| Offset | Size | Type | Meaning |
|---:|---:|---|---|
| 0 | 4 | u32 | Zero-based record index into `stars.bin` |

The entries are a permutation of every integer in `[0, star_count)`: no index
may be missing, duplicated, or out of range. They are sorted by the referenced
system name after Unicode lowercase conversion equivalent to Rust
`str::to_lowercase()`, in ascending lexicographic order. Names whose lowercase
forms compare equal are ordered by ascending star-record index. No Unicode
normalization is performed. EDGX names must be valid UTF-8; ASCII letters use
their ordinary case-insensitive ordering.

Exact lookup lowercases the requested name and binary-searches this array.
Autocomplete lowercases the requested prefix, lower-bound searches for it,
then walks forward while referenced lowercase names begin with that prefix.
Current EDDA callers trim leading and trailing whitespace from lookup input;
that input convenience is not part of the stored-name collation rule.

External producers should use cross-implementation test vectors before
claiming EDGX compatibility.

### EDGX v3: ordering guarantees and companion distance

Version 3 keeps the v2 record layout and file set unchanged and adds three
normative rules. A v3 reader MUST also read v2 (and v1) files, selecting
behavior by the header version; a v2-only reader MUST reject v3 by the
version field alone.

**Morton cell keys.** `cells.bin` keys are the bit-interleave (Morton /
z-order) of the three biased 21-bit cell coordinates instead of v2's
axis-major concatenation. With `p(v) = (v + 2^20) mod 2^21` per axis:

```text
key = interleave3(p(cx)) << 2 | interleave3(p(cy)) << 1 | interleave3(p(cz))
```

where `interleave3` places bit `k` of its input at bit `3k` of the result.
Entries remain sorted ascending by key and looked up by binary search;
only the key computation changes. Spatial neighbours now sort near each
other, so a locality-first (ranged) download of records fetches a
neighbourhood as contiguous runs. Frozen vector: cell `(0,0,0)` has key
`0x7000_0000_0000_0000`.

**Contiguous names.** Name offsets are assigned after the spatial sort:
record `i+1`'s `name_off` MUST equal record `i`'s `name_off + name_len`,
record 0's MUST be 0, and the final record MUST end exactly at
`names.bin`'s length. A cell's records therefore map to one contiguous
span of `names.bin`, and the publication validator enforces it.

**Companion distance (record byte 13).** The v2 reserved byte carries a
log-scale bucket of the supercruise distance to the nearest scoopable
companion star within 1,500 ls (the same bodies that set flag bit 1):

```text
bucket 0        no scoopable companion within 1,500 ls (or a pre-v3 record)
bucket b, 1..63 ls in (0, 1500], b = 1 + floor(62 * ln(clamp(ls,1,1500)) / ln(1500))
decode midpoint ls = 1500 ^ ((b - 1 + 0.5) / 62)
```

The decoded midpoint is within ~13 % of the encoded distance. Writers MUST
NOT emit buckets above 63; readers MUST treat 0 as unknown. v2 files decode
as bucket 0 everywhere, which is semantically correct.

Golden vectors: `crates/ed-galaxy/fixtures/edgx-v3/*.hex`, digests in
`expected.json` beside them; built from the same three-system source as the
v2 vectors plus a K-class companion at 300 ls on Sol. The v2 vectors remain
checked in and exercised as the reader-compatibility contract.
`tools/verify-binary-fixtures.mjs` does not yet decode v3 independently
(tracked in the readiness table).

## AGG1: prefix-aggregate oracle (`agg250.bin`)

Optional derived data INSIDE a v3 sub-index directory (`boost250/
agg250.bin`), written by every `subset_cells` build and therefore swapped
and deleted with the directory; absence = oracle off. It is NOT part of
any published product and is never downloaded: clients derive it locally,
so nothing here is an interchange-compatibility surface — the format may
change with a bump of the version byte and a rebuild. Implementation and
tests: `crates/ed-galaxy/src/agg.rs`; design record: ledger item
5.4.

Levels run from the leaves up, dropping 3 bits of morton key per step
until a single root remains. The leaf level is one node per occupied
cell, PARALLEL to the sub-index's morton-ordered `cells.bin` — leaves
store no keys, which is why the file requires a v3 (morton-ordered)
index and why an aggregate is only valid for the exact index it was
built from (readers MUST ignore a file whose leaf count differs from
the cell array; `Galaxy::aggregate()` enforces this).

```text
header      "AGG1" | u8 version = 1 | u8 flags_present | u16 reserved = 0
            | u32 level_count | level_count x (u32 shift, u32 node_count)
per level   node_count x node (4 B), then -- upper levels only --
            node_count x u32 first_leaf (index of the subtree's first leaf)
node        u8 flags | u8 best_boost_class | u16 star_count (saturating)
flags       bit 0 any-scoopable (companion counts)  bit 1 any-neutron
            bit 2 any-white-dwarf                   bit 3 any-station (reserved,
            never set by today's builder -- flags_present says which are filled)
```

All integers little-endian. `shift` is the number of morton key bits
dropped at that level (0 at the leaves). A node's aligned prefix cube is
recovered from its subtree's smallest leaf key with the dropped bits
zeroed; its children are the nodes of the level below whose `first_leaf`
falls inside its leaf range. Queries are EXACT, not cell-granular:
internal nodes prune by flags and cube-sphere intersection, leaf hits
scan the sub-index's own records; `want == 0` asks for any star at all.
Pinned by a 1,000-random-sphere brute-force agreement property test and
a fuel-dark descent bound (false at depth ≤ 3, zero record reads).
Measured on routing/45: 1.5–2.4 µs per corridor-point query vs 19–140 µs
for the walked scan, ~55–80 node reads; 2.9 MB for the 474,595 occupied
boost-tier cells (the spec's ≪ 1 MB guess undercounted the occupied
volume; still a rounding error beside the 5.8 GB index).

## Benchmark and acceptance gates

The reproducible Rust benchmark is in `tools/market-snapshot-bench`. Run from
the repository root:

```text
cargo run --release --manifest-path tools/market-snapshot-bench/Cargo.toml -- .data/galaxy.sqlite3 5000000
```

On the 2026-08-29 development dataset, 5,000,000 market rows and 398 commodity
definitions produced:

| Artifact | Raw bytes | zstd level 9 bytes |
|---|---:|---:|
| Normalized SQLite | 132,640,768 | 28,614,086 |
| EBEX market payload | 170,018,523 | 20,751,623 |

Release-mode Rust decompressed and scanned the binary in 0.237 s and hydrated a
fresh, verified 5,000,000-row SQLite database in 4.179 s. The binary download
was 27.5% smaller than compressed normalized SQLite.

## External-publication readiness

“Implemented” and “published” are deliberately different states. An EBEX or
EDGX version MUST NOT be described as adopted, stable, or available to external
producers until every required row below is complete. A schema still marked
draft MUST NOT appear as a required section in a public manifest.

| Gate | EBEX container v1 | All nine implemented section schemas | EDGX v2 | EDGX v3 |
|---|---|---|---|---|
| Complete normative byte layout | Complete | Complete | Complete | Complete |
| Checked-in binary/hex golden vector and decoded expectation | Complete | Complete | Complete | Complete |
| Truncation and bounds/overflow rejection tests | Complete | Complete | Complete publication validator | Complete publication validator |
| Bad-checksum rejection test | Complete | Covered by container | Covered by manifest SHA-256 | Covered by manifest SHA-256 |
| Bad-UTF-8 rejection test | Not applicable | Complete | Complete | Complete |
| Unknown required/optional section behavior test | Complete | Not applicable | Not applicable | Not applicable |
| Unsupported version/schema rejection test | Complete | Complete | Complete | Complete |
| Deterministic encode/decode test | Complete | Complete | Complete | Complete |
| SQLite/PostgreSQL freshness and deletion parity test | Not applicable | Implemented; PostgreSQL execution pending | Not applicable | Not applicable |
| Independent decoder | Complete | Complete | Complete | Pending |

The byte contracts and cross-language fixtures are ready for implementation
review. Production EBEX artifact publication remains blocked until the
environment-gated SQLite/PostgreSQL parity test completes successfully against
a disposable PostgreSQL database.
The word “Complete” above means a checked-in automated test exists; prose or a
successful development run does not count. “Partial” means relevant coverage
exists but does not yet exercise the complete normative behavior.

Publication evidence:

- `crates/ed-ebex/fixtures/ebex-v1-market.hex`: SHA-256
  `c1e5324ce6caa709b4d82e1f4f015277f2297d3dd9e7efb6211fbcaa42b6623f`
  (uncompressed golden bytes; the value is asserted inside the fixture's
  `.json` expectation).
- `crates/ed-ebex/fixtures/ebex-v1-full.hex`: SHA-256
  `73e24dfeec15f1e1387fccc3103b821e805fea3818499236349700a1a6e657a5` — nine
  sections including stars (with a provisional negative address, pinning
  signed sort order) and the production required-bit policy (markets only).
- `crates/ed-galaxy/fixtures/edgx-v2/expected.json` and
  `crates/ed-galaxy/fixtures/edgx-v3/expected.json` record each version's four
  EDGX file digests.
- `cargo test -p ed-ebex` verifies EBEX exact bytes, decoded fields, malformed
  input rejection, schema policy, encoder/decoder round trips of every
  auxiliary layout, and deterministic round trips.
- `cargo test -p ed-galaxy --lib` verifies EDGX exact bytes, decoded records,
  lookup behavior, deterministic construction, and full publication validation.
- `node tools/verify-binary-fixtures.mjs` independently decodes every current
  EBEX section — stars included — and all four EDGX files without using the
  Rust implementations.
- `cargo test -p ed-api --test postgres -- --ignored` runs the remaining parity
  contract when `EDDA_API_TEST_DATABASE_URL` names a disposable migrated test
  database. It applies baseline, replacement/deletion, stale, and newer
  operations to both stores and compares normalized results.

Future section schemas must be added as new columns or in a successor readiness
table and pass the same gates before a publisher marks them required.

## Addendum 2026-09-04: station details and confiscation sections

Two OPTIONAL sections join the community/daily EBEX plan (old readers
skip unknown optional sections by design; formats are internal until
the 1.0.0 cleanup pass, so this addendum may still be reshaped freely).

### Section 10 — station details (schema 1, 26-byte records)

Everything the Docked-event ingest learns that the identity sections
lack. One record per station the server has identity for, sorted by
station id:

    u64  station_id        (= market id, as everywhere)
    u32  flags             bit0 IS_CARRIER (authoritative: StationType),
                           bit1 HAS_BLACK_MARKET,
                           bit2 HAS_PADS, bit3 HAS_ARRIVAL
    u16  pad_small / u16 pad_medium / u16 pad_large   (counts; HAS_PADS)
    f32  arrival_ls        (HAS_ARRIVAL)
    u32  type_id           0 = unknown, else the section string table

Auxiliary: a standard string table of station type names.

### Section 11 — confiscation pairs (schema 1, 12-byte records)

    u64  station_id
    u32  commodity_id      the SAME artifact's commodity catalog id

Sorted by (station_id, commodity_id). Auxiliary: an empty string
table. Clients replace a station's prohibition list wholesale when the
station appears; absent stations keep what they have. Deliberately
lean (maintainer ruling: start lean, fatten as it grows) — candidate future
fields ride schema bumps, not guesses.
