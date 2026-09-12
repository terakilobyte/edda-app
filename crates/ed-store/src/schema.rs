//! Database schema and migration.
//!
//! Two layers, deliberately (see `docs/PLAN.md` §4.3):
//!
//! 1. `events` -- an append-only log, one row per journal line, keyed on
//!    `(file, offset)`. This is the source of truth.
//! 2. Derived tables -- materialised *from* `events`, never written directly
//!    by the ingest path.
//!
//! Keeping the raw log is what makes a derivation bug recoverable: when the
//! interpretation of an event turns out to be wrong (and the merit formula
//! work says it will be), the fix is a re-derive from local rows, not a
//! re-parse of the journal folder and not a lost history.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Bump when the DDL below changes in a way that existing rows can't satisfy.
/// A bump triggers a rebuild of the derived tables from `events`; it does NOT
/// require re-reading the journal files, which is the whole point of keeping
/// the event log.
pub const SCHEMA_VERSION: i64 = 6;

const DDL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- ── Ingest bookkeeping ──────────────────────────────────────────────
-- One row per journal file. `offset` is how many bytes we have consumed,
-- and only ever counts COMPLETE lines -- a tail can catch a half-written
-- line mid-flush, so the remainder is left for the next read.
CREATE TABLE IF NOT EXISTS journal_files (
    name       TEXT PRIMARY KEY,
    size       INTEGER NOT NULL,
    offset     INTEGER NOT NULL,
    last_ts    TEXT,
    updated_at TEXT NOT NULL
);

-- Whole-file JSON companions (Status.json, Cargo.json, ...). Unlike the
-- journal these are rewritten in full on every change, so they are stored
-- as a latest-wins snapshot rather than tailed.
CREATE TABLE IF NOT EXISTS snapshots (
    name  TEXT PRIMARY KEY,
    ts    TEXT,
    mtime INTEGER,
    raw   TEXT NOT NULL
);

-- ── The event log ───────────────────────────────────────────────────
-- (file, offset) is the natural key: it dedupes a re-read of the same file
-- while preserving genuinely repeated events, which matters because the
-- journal really does contain identical consecutive entries.
CREATE TABLE IF NOT EXISTS events (
    file           TEXT    NOT NULL,
    offset         INTEGER NOT NULL,
    ts             TEXT    NOT NULL,
    event          TEXT    NOT NULL,
    system_address INTEGER,
    market_id      INTEGER,
    raw            TEXT    NOT NULL,
    PRIMARY KEY (file, offset)
);

CREATE INDEX IF NOT EXISTS idx_events_event_ts ON events(event, ts);
CREATE INDEX IF NOT EXISTS idx_events_ts       ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_system   ON events(system_address) WHERE system_address IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_market   ON events(market_id)      WHERE market_id IS NOT NULL;

-- ── Derived: latest-wins ────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS materials (
    symbol TEXT PRIMARY KEY,
    count  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cargo (
    symbol TEXT PRIMARY KEY,
    count  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS engineers (
    name          TEXT PRIMARY KEY,
    engineer_id   INTEGER,
    progress      TEXT,
    rank          INTEGER,
    rank_progress INTEGER,
    ts            TEXT
);

CREATE TABLE IF NOT EXISTS loadout (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    ts             TEXT,
    ship           TEXT,
    ship_name      TEXT,
    ship_ident     TEXT,
    cargo_capacity INTEGER,
    unladen_mass   REAL,
    max_jump_range REAL,
    hull_value     INTEGER,
    modules        TEXT
);

CREATE TABLE IF NOT EXISTS location (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    ts                TEXT,
    system_name       TEXT,
    system_address    INTEGER,
    docked            INTEGER NOT NULL DEFAULT 0,
    station_name      TEXT,
    station_type      TEXT,
    system_security   TEXT,
    system_allegiance TEXT,
    population        INTEGER
);

-- Next jump, from FSDTarget. StarClass drives the scoopable warning
-- (KGBFOAM are scoopable; everything else is not).
CREATE TABLE IF NOT EXISTS nav (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    ts              TEXT,
    target_system   TEXT,
    system_address  INTEGER,
    star_class      TEXT,
    remaining_jumps INTEGER
);

-- ── Derived: append-only ────────────────────────────────────────────
-- Keyed on the originating (file, offset) so a re-derive is idempotent
-- without needing to clear the table first.
CREATE TABLE IF NOT EXISTS powerplay_observations (
    file              TEXT    NOT NULL,
    offset            INTEGER NOT NULL,
    ts                TEXT    NOT NULL,
    system_address    INTEGER,
    system_name       TEXT    NOT NULL,
    controlling_power TEXT,
    powerplay_state   TEXT,
    control_progress  REAL,
    reinforcement     INTEGER,
    undermining       INTEGER,
    PRIMARY KEY (file, offset)
);

CREATE INDEX IF NOT EXISTS idx_pp_system ON powerplay_observations(system_name);
CREATE INDEX IF NOT EXISTS idx_pp_ts     ON powerplay_observations(ts);

CREATE TABLE IF NOT EXISTS sales (
    file           TEXT    NOT NULL,
    offset         INTEGER NOT NULL,
    ts             TEXT    NOT NULL,
    market_id      INTEGER,
    commodity      TEXT    NOT NULL,
    count          INTEGER NOT NULL,
    sell_price     INTEGER,
    total_sale     INTEGER,
    avg_price_paid INTEGER,
    PRIMARY KEY (file, offset)
);

CREATE INDEX IF NOT EXISTS idx_sales_ts ON sales(ts);

CREATE TABLE IF NOT EXISTS merit_events (
    file          TEXT    NOT NULL,
    offset        INTEGER NOT NULL,
    ts            TEXT    NOT NULL,
    power         TEXT,
    merits_gained INTEGER,
    total_merits  INTEGER,
    PRIMARY KEY (file, offset)
);

CREATE INDEX IF NOT EXISTS idx_merits_ts ON merit_events(ts);

-- ── Combat ──────────────────────────────────────────────────────────
-- Append-only, keyed on the originating event so a re-derive is idempotent.
CREATE TABLE IF NOT EXISTS combat_kills (
    file             TEXT    NOT NULL,
    offset           INTEGER NOT NULL,
    ts               TEXT    NOT NULL,
    kind             TEXT    NOT NULL,  -- bounty | faction_kill_bond | capship_bond | pvp
    target_ship      TEXT,
    pilot_name       TEXT,
    faction          TEXT,
    victim_faction   TEXT,
    reward           INTEGER,
    system_name      TEXT,
    PRIMARY KEY (file, offset)
);

CREATE INDEX IF NOT EXISTS idx_kills_ts   ON combat_kills(ts);
CREATE INDEX IF NOT EXISTS idx_kills_kind ON combat_kills(kind);

-- Things that happened TO the commander, rather than rewards earned.
CREATE TABLE IF NOT EXISTS combat_incidents (
    file          TEXT    NOT NULL,
    offset        INTEGER NOT NULL,
    ts            TEXT    NOT NULL,
    kind          TEXT    NOT NULL,  -- died | interdicted | interdiction | escaped_interdiction
    opponent      TEXT,
    opponent_ship TEXT,
    is_player     INTEGER,
    submitted     INTEGER,
    faction       TEXT,
    system_name   TEXT,
    PRIMARY KEY (file, offset)
);

CREATE INDEX IF NOT EXISTS idx_incidents_ts ON combat_incidents(ts);

-- The route the commander is following (plotted here or imported), with
-- the cursor the watcher advances on each FSDJump. One at a time.
CREATE TABLE IF NOT EXISTS active_route (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    json    TEXT    NOT NULL,   -- ed_galaxy::router::Route
    next    INTEGER NOT NULL DEFAULT 1,
    source  TEXT,
    updated TEXT
);

-- The trade-route layer ABOVE active_route (trade-follow, 2026-09-05):
-- a cyclic list of buy/sell stops. The jump route in active_route is
-- replaced once per stop (source "trade"); this row outlives each.
CREATE TABLE IF NOT EXISTS trade_route (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    json    TEXT    NOT NULL,   -- trade_follow::TradeRoute
    updated TEXT
);

-- The carrier's route (Item 52 C, maintainer's item 6: "carrier movement takes
-- time, this needs to be longer lived"): one plan, a cursor, advanced by
-- CarrierJumpRequest / CarrierJump / CarrierLocation across sessions.
CREATE TABLE IF NOT EXISTS carrier_route (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    json    TEXT    NOT NULL,   -- carrier_follow::CarrierPlan
    updated TEXT
);

-- When EDDA's trade follower was active (maintainer, 2026-09-09: "we can only
-- measure when actively following a trade route"): the journal inside
-- these windows is where the pilot's own leg timing is sampled. `ended`
-- NULL = still following (or the app died mid-follow; read as "now").
CREATE TABLE IF NOT EXISTS trade_follow_windows (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    started TEXT NOT NULL,
    ended   TEXT
);

-- The commander's own bookmarks (maintainer, 2026-09-06: "mark iridium here
-- and EDDA privately notes it", widened the same day to "really it
-- should be any bookmark for a thing. The more granular position we can
-- get the better."). A mining spot, a crash site, a good pad, a view —
-- anything worth coming back to.
--
-- LOCAL ONLY, permanently — the shared leg was buried on the
-- poison-data premise (ledger). Lives in the main database, never the
-- galaxy attach: a galaxy re-import must not eat a commander's own
-- breadcrumbs.
--
-- Position is recorded at the FINEST grain the journal offered at the
-- moment of marking, and no finer: system always; body and station when
-- the game says where; latitude/longitude only while the surface fix is
-- live (Status.json's HAS_LAT_LONG). A null is "the game never told us",
-- never a guess — coming back to the wrong crater is worse than being
-- told to search.
CREATE TABLE IF NOT EXISTS user_marks (
    id          INTEGER PRIMARY KEY,
    label       TEXT    NOT NULL,   -- what is here: "Iridium", "Gold hotspot"…
    system      TEXT    NOT NULL,
    system_id64 INTEGER,
    body        TEXT,
    note        TEXT,
    created_ts  TEXT    NOT NULL,
    station     TEXT,
    latitude    REAL,
    longitude   REAL
);
CREATE INDEX IF NOT EXISTS idx_marks_label ON user_marks(label);

-- EDSM sphere sweeps the knowledge module has already asked for, so a
-- cell is not queried again within its revisit window. Bookkeeping about
-- what *we* fetched, so it lives with the journal rather than the galaxy.
CREATE TABLE IF NOT EXISTS edsm_sweeps (
    cell       TEXT PRIMARY KEY,
    fetched_at TEXT NOT NULL,
    systems    INTEGER NOT NULL DEFAULT 0,
    learned    INTEGER NOT NULL DEFAULT 0
);

-- Item 52 A: the commander's own or squadron carrier, replayed from the
-- Carrier* events by carrier::rebuild. Every value carries the timestamp
-- it was true at; CarrierStats only lands when Carrier Management opens.
CREATE TABLE IF NOT EXISTS carriers (
    carrier_id          INTEGER PRIMARY KEY,
    carrier_type        TEXT,
    callsign            TEXT,
    name                TEXT,
    owned               INTEGER NOT NULL DEFAULT 0,
    decommissioned      INTEGER NOT NULL DEFAULT 0,
    system_name         TEXT,
    system_address      INTEGER,
    body                TEXT,
    location_ts         TEXT,
    fuel_t              INTEGER,
    fuel_ts             TEXT,
    capacity_total      INTEGER,
    capacity_used       INTEGER,
    free_space          INTEGER,
    stats_ts            TEXT,
    jump_range_curr     REAL,
    jump_range_max      REAL,
    docking_access      TEXT,
    balance_cr          INTEGER,
    services            TEXT,
    pending_jump_system TEXT,
    pending_jump_body   TEXT,
    pending_departure   TEXT,
    pending_jump_ts     TEXT
);

-- Item 53: where each stored ship sits, from StoredShips snapshots and
-- ShipyardTransfer (the only source for a moving ship's destination).
CREATE TABLE IF NOT EXISTS ship_locations (
    ship_id      INTEGER PRIMARY KEY,
    ship_type    TEXT,
    name         TEXT,
    system_name  TEXT,
    station_name TEXT,
    market_id    INTEGER,
    in_transit   INTEGER NOT NULL DEFAULT 0,
    arrival_ts   TEXT,
    as_of        TEXT NOT NULL
);

-- What the COMMANDER moved aboard (CargoTransfer tocarrier minus
-- toship/tosrv, floored at zero): "what you moved", never "what is aboard".
CREATE TABLE IF NOT EXISTS carrier_hold (
    carrier_id INTEGER NOT NULL,
    commodity  TEXT NOT NULL,
    count      INTEGER NOT NULL,
    ts         TEXT,
    PRIMARY KEY (carrier_id, commodity)
);

-- ── Galaxy (Spansh bulk dump + EDDN) ────────────────────────────────
-- Distinct from the journal tables above: these describe the galaxy at
-- large and are only ever as fresh as the last player to visit. Journal
-- data wins on conflict -- it is first-hand and timestamped.

"#;

/// Index `sys_market.updated` so a galaxy-wide "fresh rows only" scan is
/// cheap. Kept out of the DDL because building it over ~100M rows takes
/// minutes, which must not happen inside `Store::open` on the UI's critical
/// path; callers run this from a background thread after startup. Returns
/// whether it had to build anything.
pub fn ensure_market_index(conn: &Connection) -> Result<bool> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM galaxy.sqlite_master WHERE type = 'index' AND name = 'idx_mkt_updated'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if exists {
        return Ok(false);
    }
    let started = std::time::Instant::now();
    tracing::info!(
        "building sys_market(updated) index; wide trade searches are slow until this finishes"
    );
    conn.execute_batch("CREATE INDEX IF NOT EXISTS galaxy.idx_mkt_updated ON sys_market(updated)")
        .context("creating idx_mkt_updated")?;
    tracing::info!(
        secs = started.elapsed().as_secs(),
        "sys_market(updated) index built"
    );
    Ok(true)
}

/// Whether the fresh-rows index is available for wide searches.
pub fn has_market_index(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM galaxy.sqlite_master WHERE type = 'index' AND name = 'idx_mkt_updated'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// The Spansh-derived tables live in their own file, attached as `galaxy`,
/// so bulk imports never hold the journal database's write lock and the
/// whole thing can be thrown away and rebuilt. Every statement here is
/// schema-qualified so it cannot land in `main` by accident.
pub const GALAXY_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS galaxy.sys_meta (
    key   TEXT PRIMARY KEY,
    value INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS galaxy.sys_systems (
    id64              INTEGER PRIMARY KEY,
    name              TEXT,
    x REAL, y REAL, z REAL,
    allegiance        TEXT,
    government        TEXT,
    primary_economy   TEXT,
    secondary_economy TEXT,
    security          TEXT,
    population        INTEGER,
    controlling_power TEXT,
    power_state       TEXT,
    powers            TEXT,
    updated           INTEGER,   -- epoch seconds; see ed_domain::freshness
    eddn_updated      INTEGER
);

CREATE INDEX IF NOT EXISTS galaxy.idx_sys_name ON sys_systems(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS galaxy.idx_sys_power ON sys_systems(controlling_power) WHERE controlling_power IS NOT NULL;
-- No spatial index in stock SQLite, so "systems within N ly" is a bounding-box
-- scan on x then a distance filter. Indexing x alone is what makes that cheap.
CREATE INDEX IF NOT EXISTS galaxy.idx_sys_x ON sys_systems(x);

CREATE TABLE IF NOT EXISTS galaxy.sys_stations (
    id                  INTEGER PRIMARY KEY,
    system_id64         INTEGER NOT NULL,
    name                TEXT,
    type                TEXT,
    distance_to_arrival REAL,
    primary_economy     TEXT,
    government          TEXT,
    controlling_faction TEXT,
    pad_large           INTEGER,
    pad_medium          INTEGER,
    pad_small           INTEGER,
    has_market          INTEGER NOT NULL DEFAULT 0,
    has_outfitting      INTEGER NOT NULL DEFAULT 0,
    has_shipyard        INTEGER NOT NULL DEFAULT 0,
    -- Carriers move. A route planned through one can evaporate, so callers
    -- must be able to exclude them rather than find out in flight.
    is_carrier          INTEGER NOT NULL DEFAULT 0,
    has_material_trader INTEGER NOT NULL DEFAULT 0,
    updated             INTEGER,   -- epoch seconds; see ed_domain::freshness
    outfitting_updated  INTEGER,
    shipyard_updated    INTEGER
);

CREATE INDEX IF NOT EXISTS galaxy.idx_stn_system ON sys_stations(system_id64);
CREATE INDEX IF NOT EXISTS galaxy.idx_stn_name ON sys_stations(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS galaxy.idx_stn_market ON sys_stations(has_market) WHERE has_market = 1;

-- Keyed on the internal symbol, never the display name -- same reasoning as
-- ed_journal::catalog.
-- Item metadata is stored once, not repeated at every station. At current
-- dump scale this removes hundreds of millions of duplicate strings.
CREATE TABLE IF NOT EXISTS galaxy.sys_commodities (
    id       INTEGER PRIMARY KEY,
    symbol   TEXT NOT NULL UNIQUE,
    name     TEXT,
    category TEXT
);

CREATE TABLE IF NOT EXISTS galaxy.sys_market (
    station_id INTEGER NOT NULL,
    commodity_id INTEGER NOT NULL,
    buy_price  INTEGER,
    sell_price INTEGER,
    demand     INTEGER,
    supply     INTEGER,
    updated    INTEGER,
    PRIMARY KEY (station_id, commodity_id)
) WITHOUT ROWID;

-- The per-station market watermark: the timestamp of the last snapshot
-- APPLIED to the station, whether from EDDN, an EBEX baseline or a dump.
-- Kept separately from the rows because a snapshot may legitimately be
-- empty -- after which MAX(sys_market.updated) is NULL and could not stop an
-- older replay. Freshness rule: ed_domain::freshness::accept.
CREATE TABLE IF NOT EXISTS galaxy.sys_market_watermarks (
    station_id  INTEGER PRIMARY KEY,
    observed_at INTEGER NOT NULL
) WITHOUT ROWID;

-- Per-commodity price statistics, refreshed after each hydrate: the
-- baseline for the poisoned-board guard (2026-09-04 market finding F1 -
-- RETRACTED 2026-09-04/06: the "poison" was ONE REAL station (Metz
-- Enterprise, Ega; unlimited-demand convention). Kept only for the
-- carrier envelope. Original text: one bogus NPC board at 999999
-- demand and >3x the mean topped every
-- search; the measured signature was 31 rows in 99.8M). Readers require
-- boards >= 100 before trusting a mean.
-- Station columns describe NON-carrier boards with the poison signature
-- excluded: the envelope carrier prices must fall inside to be
-- considered (maintainer rule 2026-09-04: nothing outside one standard
-- deviation of the extreme STATION prices). NULL until the first
-- refresh; station_boards = 0 disables the envelope.
CREATE TABLE IF NOT EXISTS galaxy.sys_commodity_stats (
    commodity_id     INTEGER PRIMARY KEY,
    mean_sell        REAL    NOT NULL,
    boards           INTEGER NOT NULL,
    station_sell_min REAL,
    station_sell_max REAL,
    station_sell_std REAL,
    station_buy_min  REAL,
    station_buy_max  REAL,
    station_buy_std  REAL,
    station_boards   INTEGER
) WITHOUT ROWID;

-- Trade queries run "who buys X high / sells X cheap", so symbol leads.
CREATE INDEX IF NOT EXISTS galaxy.idx_mkt_sell ON sys_market(commodity_id, sell_price DESC);
CREATE INDEX IF NOT EXISTS galaxy.idx_mkt_buy ON sys_market(commodity_id, buy_price)
    WHERE buy_price > 0 AND supply > 0;

CREATE TABLE IF NOT EXISTS galaxy.sys_modules (
    id       INTEGER PRIMARY KEY,
    symbol   TEXT NOT NULL UNIQUE,
    name     TEXT,
    class    INTEGER,
    rating   TEXT,
    category TEXT,
    ship     TEXT
);

CREATE TABLE IF NOT EXISTS galaxy.sys_outfitting (
    station_id INTEGER NOT NULL,
    module_id  INTEGER NOT NULL,
    PRIMARY KEY (station_id, module_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS galaxy.sys_ships (
    id     INTEGER PRIMARY KEY,
    symbol TEXT NOT NULL UNIQUE,
    name   TEXT
);

CREATE TABLE IF NOT EXISTS galaxy.sys_shipyard (
    station_id INTEGER NOT NULL,
    ship_id    INTEGER NOT NULL,
    PRIMARY KEY (station_id, ship_id)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS galaxy.idx_outfit_symbol ON sys_outfitting(module_id);

-- Every service a station lists (Interstellar Factors, Universal
-- Cartographics, Black Market, ...), one row each, so "nearest X" is a join.
CREATE TABLE IF NOT EXISTS galaxy.sys_station_services (
    station_id INTEGER NOT NULL,
    service    TEXT    NOT NULL,
    PRIMARY KEY (station_id, service)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS galaxy.idx_station_service ON sys_station_services(service);

-- Goods a station's market confiscates; the profit finder must not sell there.
CREATE TABLE IF NOT EXISTS galaxy.sys_market_prohibited (
    station_id INTEGER NOT NULL,
    symbol     TEXT    NOT NULL,
    PRIMARY KEY (station_id, symbol)
) WITHOUT ROWID;

-- Minor factions per system with influence and BGS states: the input for
-- high-grade-emission hunting (Outbreak, War, Boom...) and mission choice.
CREATE TABLE IF NOT EXISTS galaxy.sys_factions (
    system_id64   INTEGER NOT NULL,
    name          TEXT    NOT NULL,
    allegiance    TEXT,
    government    TEXT,
    influence     REAL,
    state         TEXT,
    active_states TEXT,          -- JSON array of state names
    updated       INTEGER,
    PRIMARY KEY (system_id64, name)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS galaxy.idx_faction_state ON sys_factions(state);

-- Bodies: landable planets with surface materials and signals, and rings
-- with mining hotspots. Only what sourcing and mining questions need.
CREATE TABLE IF NOT EXISTS galaxy.sys_bodies (
    id64                INTEGER PRIMARY KEY,
    system_id64         INTEGER NOT NULL,
    body_id             INTEGER,
    name                TEXT,
    type                TEXT,
    sub_type            TEXT,
    is_landable         INTEGER NOT NULL DEFAULT 0,
    distance_to_arrival REAL,
    gravity             REAL,
    atmosphere          TEXT,
    volcanism           TEXT,
    bio_signals         INTEGER,
    geo_signals         INTEGER,
    updated             INTEGER
);
CREATE INDEX IF NOT EXISTS galaxy.idx_bodies_system ON sys_bodies(system_id64);

CREATE TABLE IF NOT EXISTS galaxy.sys_body_materials (
    body_id64 INTEGER NOT NULL,
    material  TEXT    NOT NULL,
    percent   REAL    NOT NULL,
    PRIMARY KEY (body_id64, material)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS galaxy.idx_body_material ON sys_body_materials(material, percent DESC);

CREATE TABLE IF NOT EXISTS galaxy.sys_rings (
    body_id64    INTEGER NOT NULL,
    name         TEXT    NOT NULL,
    type         TEXT,
    mass         REAL,
    inner_radius REAL,
    outer_radius REAL,
    PRIMARY KEY (body_id64, name)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS galaxy.sys_ring_hotspots (
    body_id64 INTEGER NOT NULL,
    ring_name TEXT    NOT NULL,
    material  TEXT    NOT NULL,
    count     INTEGER NOT NULL,
    updated   INTEGER,
    PRIMARY KEY (body_id64, ring_name, material)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS galaxy.idx_hotspot_material ON sys_ring_hotspots(material);
CREATE INDEX IF NOT EXISTS galaxy.idx_shipyard_symbol ON sys_shipyard(ship_id);

-- Main-star classes learned for systems the star index has as unknown
-- (Spansh /system lookups, the commander's own scans). Loaded over the
-- index at startup so routing and hop lists use them.
CREATE TABLE IF NOT EXISTS galaxy.star_overrides (
    id64       INTEGER PRIMARY KEY,
    name       TEXT,
    subtype    TEXT,
    class      TEXT    NOT NULL,   -- ed_galaxy::StarClass letter/name
    scoopable  INTEGER NOT NULL DEFAULT 0,
    source     TEXT,
    fetched_at TEXT
);
"#;

/// Where the galaxy file lives: next to the journal database.
pub fn galaxy_path(db_path: &std::path::Path) -> std::path::PathBuf {
    db_path.with_file_name("galaxy.sqlite3")
}

/// Attach the galaxy database (created if missing) as `galaxy`, apply its
/// DDL and column migrations, and drop any `sys_*` tables left in `main`
/// from before the split -- unqualified `sys_*` names must resolve to the
/// attached file, and SQLite searches `main` first.
///
/// `path` `None` attaches an in-memory galaxy (tests).
pub fn attach_galaxy(conn: &Connection, path: Option<&std::path::Path>) -> Result<()> {
    match path {
        Some(p) => conn.execute(
            "ATTACH DATABASE ?1 AS galaxy",
            [p.to_string_lossy().as_ref()],
        )?,
        None => conn.execute("ATTACH DATABASE ':memory:' AS galaxy", [])?,
    };
    let has_market: bool = conn
        .prepare("SELECT 1 FROM galaxy.sqlite_master WHERE type='table' AND name='sys_market'")?
        .exists([])?;
    if has_market {
        let compact: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('sys_market','galaxy') WHERE name='commodity_id'",
            )?
            .exists([])?;
        anyhow::ensure!(
            compact,
            "the galaxy database uses EDDA's legacy storage format; keep it as a backup and rebuild galaxy.sqlite3 for the compact format"
        );
    }
    if path.is_some() {
        // Read-only connections cannot change the journal mode; ignore.
        let _ = conn
            .execute_batch("PRAGMA galaxy.journal_mode = WAL; PRAGMA galaxy.synchronous = NORMAL;");
    }
    // Read-only connections (the app's readers) only need the attachment;
    // the DDL is a no-op for them and fails with "readonly database".
    if let Err(e) = conn.execute_batch(GALAXY_DDL) {
        if e.to_string().contains("readonly") {
            return Ok(());
        }
        return Err(e).context("applying galaxy DDL");
    }
    // Columns added after the first release; the DDL only creates tables.
    for (table, column, decl) in [
        (
            "sys_stations",
            "has_material_trader",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("sys_stations", "economies", "TEXT"), // JSON object {economy: share}
        ("sys_stations", "state", "TEXT"),     // controlling faction's state at the station
        ("sys_stations", "body_name", "TEXT"), // surface ports: the body they sit on
        ("sys_stations", "outfitting_updated", "INTEGER"),
        ("sys_stations", "shipyard_updated", "INTEGER"),
        ("sys_systems", "eddn_updated", "INTEGER"),
        ("sys_commodity_stats", "station_sell_min", "REAL"),
        ("sys_commodity_stats", "station_sell_max", "REAL"),
        ("sys_commodity_stats", "station_sell_std", "REAL"),
        ("sys_commodity_stats", "station_buy_min", "REAL"),
        ("sys_commodity_stats", "station_buy_max", "REAL"),
        ("sys_commodity_stats", "station_buy_std", "REAL"),
        ("sys_commodity_stats", "station_boards", "INTEGER"),
        // Docked-event identity watermark (station identity ingest,
        // 2026-09-04): pads/carrier/type/arrival freshness, separate
        // from the market watermark.
        ("sys_stations", "identity_updated", "INTEGER"),
    ] {
        let has_col: bool = conn
            .prepare(&format!(
                "SELECT 1 FROM pragma_table_info('{table}', 'galaxy') WHERE name = '{column}'"
            ))?
            .exists([])?;
        if !has_col {
            conn.execute(
                &format!("ALTER TABLE galaxy.{table} ADD COLUMN {column} {decl}"),
                [],
            )?;
        }
    }

    migrate_galaxy(conn)?;

    // Pre-split leftovers in main shadow the attached tables; drop them.
    let legacy: Vec<String> = conn
        .prepare("SELECT name FROM main.sqlite_master WHERE type IN ('table','index') AND (name LIKE 'sys\\_%' ESCAPE '\\' OR name LIKE 'idx\\_%' ESCAPE '\\') AND name NOT LIKE 'idx\\_events%' ESCAPE '\\' AND name NOT LIKE 'idx\\_pp%' ESCAPE '\\' AND name NOT LIKE 'idx\\_sales%' ESCAPE '\\' AND name NOT LIKE 'idx\\_merits%' ESCAPE '\\' AND name NOT LIKE 'idx\\_kills%' ESCAPE '\\' AND name NOT LIKE 'idx\\_incidents%' ESCAPE '\\'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let legacy_tables: Vec<&String> = legacy.iter().filter(|n| n.starts_with("sys_")).collect();
    if !legacy_tables.is_empty() {
        tracing::info!(tables = legacy_tables.len(), "dropping pre-split galaxy tables from the journal database (re-import to refill; VACUUM to reclaim space)");
        for t in legacy_tables {
            conn.execute(&format!("DROP TABLE IF EXISTS main.\"{t}\""), [])?;
        }
    }
    Ok(())
}

/// Galaxy file version, recorded in `PRAGMA galaxy.user_version`.
///
/// * 2 -- compact inventory keys (`commodity_id` rather than symbol).
/// * 3 -- every `*updated` column is an integer epoch; market watermarks.
/// * 4 -- `sys_systems.security` holds display names, never journal symbols.
/// * 5 -- one commodity row per good: wrapper/spaced variants folded into
///   their canonical symbol (census 2026-09-05; inflow sealed in 0.2.3).
/// * 6 -- provisional system ghosts merged into their real rows (the
///   vanishing-station field case, 2026-09-05).
pub const GALAXY_VERSION: i64 = 6;

/// Galaxy tables whose `updated` column was declared TEXT before v3 and
/// held whichever timestamp string the source produced.
const EPOCH_MIGRATED_TABLES: [&str; 6] = [
    "sys_systems",
    "sys_stations",
    "sys_factions",
    "sys_bodies",
    "sys_ring_hotspots",
    // Declared INTEGER from the start, but a row bound as text stays text
    // and breaks every integer read downstream; a full scan once is cheap
    // next to that (about 1 min per 50 M rows).
    "sys_market",
];

/// Bring an attached galaxy up to [`GALAXY_VERSION`].
///
/// v3 converts TEXT `updated` columns to integer epochs *in place*. The live
/// database is tens of gigabytes, so this is never a table rebuild: the old
/// column is renamed aside (a metadata-only operation), a new INTEGER
/// column is added (also metadata-only), and one UPDATE fills it. Strings
/// SQLite cannot parse become NULL rather than aborting the migration; the
/// Spansh `+00` suffix is widened to `+00:00` first because SQLite's
/// `strftime` will not take an hour-only offset. The renamed column is
/// emptied but not dropped, since DROP COLUMN would rewrite the table.
fn migrate_galaxy(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA galaxy.user_version", [], |r| r.get(0))?;
    if version >= GALAXY_VERSION {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for table in EPOCH_MIGRATED_TABLES {
        if version >= 3 {
            break;
        }
        let declared: Option<String> = tx
            .query_row(
                &format!("SELECT type FROM pragma_table_info('{table}', 'galaxy') WHERE name = 'updated'"),
                [],
                |r| r.get(0),
            )
            .optional()?;
        let started = std::time::Instant::now();
        // A TEXT-affinity column would turn the integers straight back into
        // text, so it is swapped for an INTEGER one first. A column that is
        // already INTEGER (a fresh file, or one that took strings by
        // accident) only needs the UPDATE.
        let sql = if declared.is_some_and(|d| d.eq_ignore_ascii_case("text")) {
            format!(
                "ALTER TABLE galaxy.{table} RENAME COLUMN updated TO updated_text;
                 ALTER TABLE galaxy.{table} ADD COLUMN updated INTEGER;
                 UPDATE galaxy.{table}
                    SET updated = {}, updated_text = NULL
                  WHERE updated_text IS NOT NULL;",
                epoch_from_text_sql("updated_text")
            )
        } else {
            format!(
                "UPDATE galaxy.{table} SET updated = {} WHERE typeof(updated) = 'text';",
                epoch_from_text_sql("updated")
            )
        };
        tx.execute_batch(&sql)
            .with_context(|| format!("converting galaxy.{table}.updated to epoch seconds"))?;
        tracing::info!(
            table,
            secs = started.elapsed().as_secs(),
            "galaxy: converted updated timestamps to epoch seconds"
        );
    }
    if version < 4 {
        // EDDN journal frames and the app's own journal path both stored the
        // raw `SystemSecurity` symbol before v4; the dump path never did.
        // The SQL twin of `ed_domain::system::security_name` for the
        // symbols the game emits.
        let changed = tx.execute(
            "UPDATE galaxy.sys_systems SET security = CASE lower(security)
                 WHEN '$system_security_low;' THEN 'Low'
                 WHEN '$system_security_medium;' THEN 'Medium'
                 WHEN '$system_security_high;' THEN 'High'
                 WHEN '$galaxy_map_info_state_anarchy;' THEN 'Anarchy'
                 WHEN '$galaxy_map_info_state_lawless;' THEN 'Lawless'
                 ELSE security END
             WHERE security LIKE '$%'",
            [],
        )?;
        tracing::info!(
            changed,
            "galaxy: security symbols rewritten as display names"
        );
    }
    if version < 5 {
        let (merged, deleted, kept) = fold_commodity_variants(&tx)?;
        tracing::info!(
            merged,
            deleted,
            kept,
            "galaxy: commodity catalog folded to canonical symbols"
        );
    }
    if version < 6 {
        let (healed, orphans) = merge_provisional_systems(&tx)?;
        tracing::info!(
            healed,
            orphans,
            "galaxy: provisional system ghosts merged into their real rows"
        );
    }
    tx.pragma_update(Some("galaxy"), "user_version", GALAXY_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// v5: fold fragmented commodity rows into one row per good. The census
/// of 2026-09-05 (production server, mirrored client-side): journal
/// wrapper symbols (`$gold_name;`) interned as their own rows, each
/// stranding market rows invisible to search; plus spaced display
/// spellings ("advanced catalysers") stranding nothing. Inflow was sealed
/// in 0.2.3 (`canonical_symbol` at every intern), so the variant set is
/// frozen — this fold retires it.
///
/// Per variant: display metadata is absorbed into the canonical row
/// (created if missing), market rows are repointed newer-wins, the
/// loser's stats row is dropped (the next stats refresh recomputes the
/// winner's), and the loser is deleted. A spaced spelling that matches no
/// canonical symbol when space-stripped is deleted only if provably
/// stranded — one that somehow gained market rows is kept, never folded
/// blind. Returns (merged, deleted, kept).
fn fold_commodity_variants(conn: &Connection) -> Result<(u64, u64, u64)> {
    let rows: Vec<(i64, String, Option<String>, Option<String>)> = conn
        .prepare("SELECT id, symbol, name, category FROM galaxy.sys_commodities")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let (mut merged, mut deleted, mut kept) = (0u64, 0u64, 0u64);
    // Wrappers first: they may CREATE canonical rows that spaced
    // spellings then match by space-stripping.
    let wrappers = rows
        .iter()
        .filter_map(|(id, symbol, name, category)| {
            let canon = crate::market::canonical_symbol(symbol);
            (canon != *symbol).then_some((*id, canon, name.as_deref(), category.as_deref()))
        })
        .collect::<Vec<_>>();
    for (loser, canon, name, category) in wrappers {
        let winner = crate::market::intern_commodity(conn, &canon, name, category)?;
        merge_market_rows(conn, loser, winner)?;
        merged += 1;
    }
    for (loser, symbol, name, category) in &rows {
        if !symbol.contains(' ') {
            continue;
        }
        let stripped: String = symbol.chars().filter(|c| !c.is_whitespace()).collect();
        let winner: Option<i64> = conn
            .prepare("SELECT id FROM galaxy.sys_commodities WHERE symbol = ?1")?
            .query_row([&stripped], |r| r.get(0))
            .optional()?;
        match winner {
            Some(winner) => {
                crate::market::intern_commodity(
                    conn,
                    &stripped,
                    name.as_deref(),
                    category.as_deref(),
                )?;
                merge_market_rows(conn, *loser, winner)?;
                merged += 1;
            }
            None => {
                let referenced = conn
                    .prepare("SELECT 1 FROM galaxy.sys_market WHERE commodity_id = ?1 LIMIT 1")?
                    .exists([loser])?;
                if referenced {
                    kept += 1;
                } else {
                    conn.execute(
                        "DELETE FROM galaxy.sys_commodity_stats WHERE commodity_id = ?1",
                        [loser],
                    )?;
                    conn.execute("DELETE FROM galaxy.sys_commodities WHERE id = ?1", [loser])?;
                    deleted += 1;
                }
            }
        }
    }
    Ok((merged, deleted, kept))
}

/// v6: merge provisional system ghosts (negative id64, minted when an
/// EDDN message named a system the store did not know yet — usually a
/// message racing the initial hydration) into their real rows where one
/// exists. The 2026-09-05 field case: a ghost "Ega" shadowed the real
/// one, the ingest re-parented Metz Enterprise onto it on the
/// commander's own dock, and the station — coordinates NULL via the
/// ghost — vanished from every radius query mid-trade-run. Stations are
/// repointed to the real id64 and the ghost deleted; a ghost with no
/// real twin is a genuine unknown and is kept. Returns (healed, kept).
pub(crate) fn merge_provisional_systems(conn: &Connection) -> Result<(u64, u64)> {
    let ghosts: Vec<(i64, String)> = conn
        .prepare("SELECT id64, name FROM galaxy.sys_systems WHERE id64 < 0")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let (mut healed, mut kept) = (0u64, 0u64);
    for (ghost, name) in ghosts {
        let real: Option<i64> = conn
            .query_row(
                "SELECT id64 FROM galaxy.sys_systems
                 WHERE name = ?1 COLLATE NOCASE AND id64 >= 0
                 ORDER BY id64 LIMIT 1",
                [&name],
                |r| r.get(0),
            )
            .optional()?;
        match real {
            Some(real) => {
                conn.execute(
                    "UPDATE galaxy.sys_stations SET system_id64 = ?2 WHERE system_id64 = ?1",
                    params![ghost, real],
                )?;
                conn.execute("DELETE FROM galaxy.sys_systems WHERE id64 = ?1", [ghost])?;
                healed += 1;
            }
            None => kept += 1,
        }
    }
    Ok((healed, kept))
}

/// Repoint a loser commodity's market rows at the winner, newer-wins on
/// collision, then remove the loser and its stats row.
fn merge_market_rows(conn: &Connection, loser: i64, winner: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO galaxy.sys_market (station_id, commodity_id, buy_price, sell_price, demand, supply, updated)
         SELECT station_id, ?2, buy_price, sell_price, demand, supply, updated
         FROM galaxy.sys_market WHERE commodity_id = ?1
         ON CONFLICT(station_id, commodity_id) DO UPDATE SET
             buy_price = excluded.buy_price,
             sell_price = excluded.sell_price,
             demand = excluded.demand,
             supply = excluded.supply,
             updated = excluded.updated
         WHERE COALESCE(excluded.updated, 0) > COALESCE(sys_market.updated, 0)",
        params![loser, winner],
    )?;
    conn.execute(
        "DELETE FROM galaxy.sys_market WHERE commodity_id = ?1",
        [loser],
    )?;
    conn.execute(
        "DELETE FROM galaxy.sys_commodity_stats WHERE commodity_id = ?1",
        [loser],
    )?;
    conn.execute("DELETE FROM galaxy.sys_commodities WHERE id = ?1", [loser])?;
    Ok(())
}

/// SQL that turns a stored timestamp (integer already, RFC-3339, or the
/// Spansh `YYYY-MM-DD HH:MM:SS+00` shape) into epoch seconds, or NULL when
/// SQLite cannot parse it. The SQL twin of
/// `ed_domain::freshness::parse_timestamp`.
fn epoch_from_text_sql(column: &str) -> String {
    format!(
        "CASE typeof({column})
             WHEN 'integer' THEN {column}
             WHEN 'text' THEN CAST(strftime('%s',
                 CASE WHEN length({column}) = 22 AND substr({column}, 20, 1) IN ('+', '-')
                      THEN {column} || ':00'
                      ELSE {column} END) AS INTEGER)
             ELSE NULL END"
    )
}

/// Drop the galaxy secondary indexes for a bulk load; `attach_galaxy` /
/// `rebuild_galaxy_indexes` puts them back. Inserting into unindexed
/// tables is several times faster; one CREATE INDEX at the end is a sort.
pub fn drop_galaxy_indexes(conn: &Connection) -> Result<()> {
    for name in galaxy_index_names() {
        conn.execute(&format!("DROP INDEX IF EXISTS galaxy.{name}"), [])?;
    }
    Ok(())
}

pub fn rebuild_galaxy_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(GALAXY_DDL)
        .context("rebuilding galaxy indexes")?;
    Ok(())
}

/// Run a bulk load with the galaxy's secondary indexes removed, rebuilding
/// them exactly once afterward. Rebuilding is attempted even when the load
/// fails (including cancellation), so an interrupted refresh does not leave
/// normal lookups on unindexed tables.
pub fn with_galaxy_indexes_dropped<T>(
    conn: &Connection,
    load: impl FnOnce() -> Result<T>,
) -> Result<T> {
    drop_galaxy_indexes(conn)?;
    let loaded = load();
    let rebuilt = rebuild_galaxy_indexes(conn);
    match (loaded, rebuilt) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(load_err), Ok(())) => Err(load_err),
        (Ok(_), Err(rebuild_err)) => Err(rebuild_err),
        (Err(load_err), Err(rebuild_err)) => {
            Err(load_err.context(format!("galaxy index rebuild also failed: {rebuild_err:#}")))
        }
    }
}

fn galaxy_index_names() -> Vec<String> {
    GALAXY_DDL
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            l.strip_prefix("CREATE INDEX IF NOT EXISTS galaxy.")
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_string)
        })
        .collect()
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(DDL).context("applying schema DDL")?;

    // Main-database columns added after a table first shipped. CREATE
    // TABLE IF NOT EXISTS never revisits an existing table, so a
    // commander who already has bookmarks needs the new position columns
    // bolted on — without touching the rows they already made.
    for (table, column, decl) in [
        ("user_marks", "station", "TEXT"),
        ("user_marks", "latitude", "REAL"),
        ("user_marks", "longitude", "REAL"),
    ] {
        let has_col: bool = conn
            .prepare(&format!(
                "SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{column}'"
            ))?
            .exists([])?;
        if !has_col {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
                [],
            )?;
        }
    }

    let current: Option<i64> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| s.parse().ok());

    match current {
        Some(v) if v == SCHEMA_VERSION => {}
        _ => {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [SCHEMA_VERSION.to_string()],
            )?;
            // Force a rebuild of derived state on next sync. The event log
            // is untouched, so this costs a re-derive and not a re-ingest.
            conn.execute(
                "DELETE FROM meta WHERE key IN ('derived_file', 'derived_offset')",
                [],
            )?;
        }
    }
    Ok(())
}

/// Schema version currently recorded in the database.
pub fn version(conn: &Connection) -> Result<i64> {
    let v: String = conn.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    Ok(v.parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_index_detection_uses_the_attached_database() {
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, None).unwrap();

        // A same-named index in main must not masquerade as the galaxy index.
        conn.execute_batch(
            "CREATE TABLE main.decoy(updated TEXT);
             CREATE INDEX main.idx_mkt_updated ON decoy(updated);",
        )
        .unwrap();
        assert!(!has_market_index(&conn));

        assert!(ensure_market_index(&conn).unwrap());
        assert!(has_market_index(&conn));
        assert!(!ensure_market_index(&conn).unwrap());
    }

    #[test]
    fn failed_bulk_load_restores_secondary_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, None).unwrap();

        let result: Result<()> =
            with_galaxy_indexes_dropped(&conn, || anyhow::bail!("simulated cancelled import"));
        assert!(result.is_err());
        let restored: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM galaxy.sqlite_master WHERE type='index' AND name='idx_sys_name')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(restored);
    }

    #[test]
    fn fresh_galaxy_uses_compact_inventory_keys() {
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, None).unwrap();
        let cols = |table: &str| {
            conn.prepare(&format!(
                "SELECT name FROM pragma_table_info('{table}','galaxy')"
            ))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        assert_eq!(
            cols("sys_market"),
            [
                "station_id",
                "commodity_id",
                "buy_price",
                "sell_price",
                "demand",
                "supply",
                "updated"
            ]
        );
        assert_eq!(cols("sys_outfitting"), ["station_id", "module_id"]);
        assert_eq!(cols("sys_shipyard"), ["station_id", "ship_id"]);
    }

    /// A galaxy written before every `*updated` column became an epoch holds
    /// timestamp strings (EDDN RFC-3339 and Spansh `YYYY-MM-DD HH:MM:SS+00`).
    /// Attaching converts them in place -- an UPDATE, never a table rebuild
    /// (the live database is tens of GB) -- and leaves garbage as NULL.
    #[test]
    fn legacy_text_updated_columns_are_migrated_to_epochs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("galaxy.sqlite3");
        // A v2 file: today's tables with TEXT-affinity `updated` columns.
        let legacy_ddl = GALAXY_DDL
            .lines()
            .map(|line| {
                let t = line.trim_start();
                if t.starts_with("updated ") && t.contains("INTEGER") && !t.contains("--") {
                    // sys_market.updated was always an epoch; leave it.
                    line.replacen("INTEGER", "TEXT", 1)
                } else if t.starts_with("updated ") && t.contains("epoch seconds") {
                    line.split("INTEGER").next().unwrap().to_string() + "TEXT,"
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .replace("    updated    TEXT,\n", "    updated    INTEGER,\n");
        {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute(
                "ATTACH DATABASE ?1 AS galaxy",
                [path.to_string_lossy().as_ref()],
            )
            .unwrap();
            conn.execute_batch(&legacy_ddl).unwrap();
            let declared: String = conn
                .query_row(
                    "SELECT type FROM pragma_table_info('sys_systems','galaxy') WHERE name='updated'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(declared, "TEXT", "fixture must be a real v2 layout");
            conn.execute_batch(
                "INSERT INTO galaxy.sys_systems (id64, name, updated) VALUES
                     (1, 'Iso', '2026-08-24T01:00:00Z'),
                     (2, 'Spansh', '2026-08-24 01:00:00+00'),
                     (3, 'Garbage', 'yesterday'),
                     (4, 'Missing', NULL);
                 INSERT INTO galaxy.sys_stations (id, system_id64, name, updated) VALUES
                     (10, 1, 'Port', '2026-08-24T01:00:00Z');
                 INSERT INTO galaxy.sys_bodies (id64, system_id64, updated) VALUES (100, 1, '2026-08-24T01:00:00Z');
                 INSERT INTO galaxy.sys_factions (system_id64, name, updated) VALUES (1, 'F', '2026-08-24T01:00:00Z');
                 INSERT INTO galaxy.sys_ring_hotspots (body_id64, ring_name, material, count, updated)
                     VALUES (100, 'A Ring', 'Painite', 2, '2026-08-24T01:00:00Z');
                 PRAGMA galaxy.user_version = 2;",
            )
            .unwrap();
        }
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, Some(&path)).unwrap();

        let epoch = 1_787_533_200i64;
        let systems: Vec<(i64, String, Option<i64>)> = conn
            .prepare("SELECT id64, typeof(updated), updated FROM galaxy.sys_systems ORDER BY id64")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            systems,
            vec![
                (1, "integer".into(), Some(epoch)),
                (2, "integer".into(), Some(epoch)),
                (3, "null".into(), None),
                (4, "null".into(), None),
            ]
        );
        for (table, key) in [
            ("sys_stations", "id = 10"),
            ("sys_bodies", "id64 = 100"),
            ("sys_factions", "name = 'F'"),
            ("sys_ring_hotspots", "material = 'Painite'"),
        ] {
            let got: i64 = conn
                .query_row(
                    &format!("SELECT updated FROM galaxy.{table} WHERE {key}"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(got, epoch, "{table}");
        }
        // The migration is recorded so it never runs twice.
        let version: i64 = conn
            .query_row("PRAGMA galaxy.user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, GALAXY_VERSION);
    }

    /// Before v4 the journal and EDDN paths stored `SystemSecurity` as the
    /// game's localisation symbol; the dump path stored display names. A v3
    /// file is rewritten in place so every reader sees one vocabulary.
    #[test]
    fn v3_security_symbols_become_display_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("galaxy.sqlite3");
        {
            let conn = Connection::open_in_memory().unwrap();
            attach_galaxy(&conn, Some(&path)).unwrap();
            conn.execute_batch(
                "INSERT INTO galaxy.sys_systems (id64, name, security) VALUES
                     (1, 'Eurybia', '$GAlAXY_MAP_INFO_state_anarchy;'),
                     (2, 'Deciat', '$SYSTEM_SECURITY_high;'),
                     (3, 'Sol', 'High'),
                     (4, 'Odd', '$SYSTEM_SECURITY_martial;'),
                     (5, 'Unknown', NULL);
                 PRAGMA galaxy.user_version = 3;",
            )
            .unwrap();
        }
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, Some(&path)).unwrap();
        let got: Vec<Option<String>> = conn
            .prepare("SELECT security FROM galaxy.sys_systems ORDER BY id64")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            got,
            vec![
                Some("Anarchy".into()),
                Some("High".into()),
                Some("High".into()),
                // Not a symbol the migration knows: left for the next writer.
                Some("$SYSTEM_SECURITY_martial;".into()),
                None,
            ]
        );
        let version: i64 = conn
            .query_row("PRAGMA galaxy.user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, GALAXY_VERSION);
    }

    /// `sys_market.updated` is declared INTEGER, but a slice copied from a
    /// pre-compact database (or any writer that bound a string) leaves TEXT
    /// values in it, and the profit finder then fails on the first such row
    /// ("Invalid column type Text"). Found with the real Wongi slice. The
    /// migration must normalise it like the other tables.
    #[test]
    fn text_market_timestamps_are_migrated_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("galaxy.sqlite3");
        {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute(
                "ATTACH DATABASE ?1 AS galaxy",
                [path.to_string_lossy().as_ref()],
            )
            .unwrap();
            conn.execute_batch(GALAXY_DDL).unwrap();
            conn.execute_batch(
                "INSERT INTO galaxy.sys_commodities (id, symbol) VALUES (1, 'gold');
                 INSERT INTO galaxy.sys_market (station_id, commodity_id, buy_price, sell_price, demand, supply, updated) VALUES
                     (10, 1, 1, 2, 3, 4, '2026-08-24 01:00:00+00'),
                     (11, 1, 1, 2, 3, 4, '2026-08-24T01:00:00Z'),
                     (12, 1, 1, 2, 3, 4, 1787533200);
                 PRAGMA galaxy.user_version = 2;",
            )
            .unwrap();
        }
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, Some(&path)).unwrap();
        let rows: Vec<(i64, String, i64)> = conn
            .prepare("SELECT station_id, typeof(updated), updated FROM galaxy.sys_market ORDER BY station_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let epoch = 1_787_533_200i64;
        assert_eq!(
            rows,
            vec![
                (10, "integer".into(), epoch),
                (11, "integer".into(), epoch),
                (12, "integer".into(), epoch),
            ]
        );
    }

    /// v5: the commodity fold, every class from the 2026-09-05 census.
    /// A wrapper variant with the newer price wins the collision; the
    /// canonical row's display name survives absorption; a matched spaced
    /// spelling merges; an unmatched stranded one is deleted; an unmatched
    /// spelling WITH market rows is kept — never folded blind.
    #[test]
    fn commodity_variants_fold_to_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("galaxy.sqlite3");
        {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute(
                "ATTACH DATABASE ?1 AS galaxy",
                [path.to_string_lossy().as_ref()],
            )
            .unwrap();
            conn.execute_batch(GALAXY_DDL).unwrap();
            conn.execute_batch(
                "INSERT INTO galaxy.sys_commodities (id, symbol, name, category) VALUES
                     (1, 'gold', 'Gold', 'Metals'),
                     (2, '$gold_name;', NULL, NULL),
                     (3, '$silver_name', 'Silver', NULL),        -- canonical row missing: fold creates it
                     (4, 'advanced catalysers', NULL, NULL),     -- spaced, matches 5 stripped
                     (5, 'advancedcatalysers', 'Advanced Catalysers', 'Chemicals'),
                     (6, 'aepyornis egg', NULL, NULL),           -- spaced, unmatched, stranded
                     (7, 'azure milk', NULL, NULL);              -- spaced, unmatched, referenced
                 INSERT INTO galaxy.sys_market (station_id, commodity_id, buy_price, sell_price, demand, supply, updated) VALUES
                     (10, 1, 0, 9000, 100, 0, 100),   -- canonical, older than the wrapper's
                     (10, 2, 0, 9500, 120, 0, 200),   -- wrapper wins station 10
                     (11, 2, 0, 9600, 130, 0, 50),    -- wrapper-only station
                     (12, 1, 0, 8000, 90, 0, 300),    -- canonical newer: wrapper must NOT win
                     (12, 2, 0, 7000, 80, 0, 250),
                     (13, 3, 0, 40000, 10, 0, 400),
                     (14, 7, 0, 1234, 5, 0, 500);
                 INSERT INTO galaxy.sys_commodity_stats (commodity_id, mean_sell, boards) VALUES
                     (1, 9000.0, 500), (2, 9500.0, 3);
                 PRAGMA galaxy.user_version = 4;",
            )
            .unwrap();
        }
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, Some(&path)).unwrap();
        let goods: Vec<(String, Option<String>)> = conn
            .prepare("SELECT symbol, name FROM galaxy.sys_commodities ORDER BY symbol")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            goods,
            vec![
                (
                    "advancedcatalysers".into(),
                    Some("Advanced Catalysers".into())
                ),
                ("azure milk".into(), None), // referenced: kept, not folded blind
                ("gold".into(), Some("Gold".into())),
                ("silver".into(), Some("Silver".into())),
            ]
        );
        let gold: Vec<(i64, i64, i64)> = conn
            .prepare(
                "SELECT station_id, sell_price, updated FROM galaxy.sys_market m
                 JOIN galaxy.sys_commodities c ON c.id = m.commodity_id
                 WHERE c.symbol = 'gold' ORDER BY station_id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            gold,
            vec![
                (10, 9500, 200), // wrapper was newer: it won
                (11, 9600, 50),  // wrapper-only station repointed
                (12, 8000, 300), // canonical was newer: wrapper lost
            ]
        );
        // The loser's stats row is gone; the winner's survives until the
        // next refresh recomputes it.
        let stats: Vec<i64> = conn
            .prepare("SELECT commodity_id FROM galaxy.sys_commodity_stats ORDER BY commodity_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(stats, vec![1]);
        let version: i64 = conn
            .query_row("PRAGMA galaxy.user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, GALAXY_VERSION);
    }

    /// v6: ghost systems (negative id64) with a real twin are merged —
    /// stations repointed, ghost deleted; a ghost with no twin is a
    /// genuine unknown and survives. Field case 2026-09-05: 41 of 73
    /// ghost-parented stations in the maintainer's live database were healable
    /// this way, Metz Enterprise (the vanished palladium sell) included.
    #[test]
    fn provisional_ghosts_merge_into_their_real_systems() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("galaxy.sqlite3");
        {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute(
                "ATTACH DATABASE ?1 AS galaxy",
                [path.to_string_lossy().as_ref()],
            )
            .unwrap();
            conn.execute_batch(GALAXY_DDL).unwrap();
            conn.execute_batch(
                "INSERT INTO galaxy.sys_systems (id64, name, x, y, z) VALUES (4923936737651, 'Ega', 7.0, 8.0, 9.0);
                 INSERT INTO galaxy.sys_systems (id64, name) VALUES (-7, 'ega');   -- case-insensitive twin
                 INSERT INTO galaxy.sys_systems (id64, name) VALUES (-8, 'Nowhere Special');
                 INSERT INTO galaxy.sys_stations (id, system_id64, name, has_market) VALUES
                     (900, -7, 'Metz Enterprise', 1),
                     (901, -8, 'Lost Outpost', 1);
                 PRAGMA galaxy.user_version = 5;",
            )
            .unwrap();
        }
        let conn = Connection::open_in_memory().unwrap();
        attach_galaxy(&conn, Some(&path)).unwrap();
        let (parent, x): (i64, Option<f64>) = conn
            .query_row(
                "SELECT st.system_id64, sy.x FROM galaxy.sys_stations st
                 JOIN galaxy.sys_systems sy ON sy.id64 = st.system_id64 WHERE st.id = 900",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            parent, 4923936737651,
            "station repointed to the real system"
        );
        assert_eq!(x, Some(7.0), "and radius queries can see it again");
        let ghosts: Vec<i64> = conn
            .prepare("SELECT id64 FROM galaxy.sys_systems WHERE id64 < 0 ORDER BY id64")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            ghosts,
            vec![-8],
            "twinless ghost survives; merged ghost is gone"
        );
        let orphan_parent: i64 = conn
            .query_row(
                "SELECT system_id64 FROM galaxy.sys_stations WHERE id = 901",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parent, -8, "the genuine unknown keeps its station");
    }

    /// DDL lives here, not in whichever app module happens to need a table.
    #[test]
    fn edsm_sweeps_table_is_part_of_the_schema() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        attach_galaxy(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO edsm_sweeps (cell, fetched_at, systems, learned) VALUES ('0:0:0', 1, 2, 3)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn legacy_galaxy_is_rejected_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("galaxy.sqlite3");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE sys_market(station_id INTEGER, symbol TEXT)")
            .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let error = attach_galaxy(&conn, Some(&path)).unwrap_err().to_string();
        assert!(error.contains("legacy storage format"));
        let old_cols: i64 = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sys_market')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_cols, 2);
    }
}
