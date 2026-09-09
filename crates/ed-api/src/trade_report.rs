//! The profit finder's data half on the server (API-only spec, Phase
//! A.2): Postgres supplies the sphere's candidate stations and every
//! fresh row at them; `ed_route::profit::assemble` does the pairing.
//! The exclusion rules here mirror `ed_route::profit::candidate_stations`
//! line for line so the two finders count the same things.
//!
//! Measured first (docs/benches/trade-rows-pull-2026-09-07.csv): the
//! uncapped 100 ly pull is 2–4 s and 160k–450k rows; capped at the
//! finder's 2,500 nearest it is ~190 ms at Sol, Deciat and Wyrd. The
//! cap is applied to candidates BEFORE the row pull, as the local
//! finder does, and the rest are counted in `excluded.beyond_station_cap`.
//!
//! Known gap (ledgered): the carrier price envelope
//! (`commodity_stats_by_symbol`) is not applied here — the server has no
//! per-commodity stats table yet. With `include_carriers: false` (the
//! default) nothing differs.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ed_domain::station::{PadSize, StationClass};
use ed_route::profit::{
    silence_prohibited, Constraints, Excluded, MarketRow, Prepared, SearchTiming, StationRef,
};
use sqlx::{PgPool, Row};

use crate::market_search::Refusal;

/// A resource guard, not a search rule. It was 2,500 (the local
/// finder's cap; ~190 ms of Postgres in the row-pull bench) applied
/// nearest-first BEFORE freshness, which in the bubble core turned a
/// 60 ly search into a 34 ly one and hid Inara's top Wongi route (maintainer,
/// 2026-09-09: "clearly the previous filter suppressed better results").
/// Candidates are now gated by price age first, which is the real
/// search space (a few hundred stations at 8 h, ~1,500 at 48 h in the
/// core); the cap only stops a 500 ly / 30-day request from pulling the
/// whole market. `reach_ly` on the report says how far the search got.
/// The nearest-N cap applied AFTER the freshness gate. 10,000 was the
/// first number and it took the box to 474 MB free within the hour
/// (2026-09-09): Sol / 100 ly / 48 h admits 2,653 fresh stations with
/// 434k rows, and pairing is quadratic in stations that actually hold a
/// board — the old cap-before-freshness let through 2,500 stations of
/// which only ~300 had rows, which is why it looked cheap. Measured on
/// the bubble cases: 125 stations pair in 107 ms, 211 in 224 ms; 1,000
/// extrapolates to ~5 s, which is the most a search may hold of the
/// trade gate. Re-measure before raising.
pub const DEFAULT_MAX_STATIONS: usize = 1_000;
pub const MAX_RADIUS_LY: f64 = 500.0;

/// The 16 station columns the candidate query selects, in order.
const STATION_COLS: &str = "st.id, st.name, sy.name, sy.address, sy.x, sy.y, sy.z, st.arrival_ls, \
                            st.pad_small, st.pad_medium, st.pad_large, COALESCE(st.is_carrier, false), st.station_type, \
                            sy.controlling_power, sy.power_state, sy.powers";

pub(crate) fn station_ref(row: &sqlx::postgres::PgRow) -> StationRef {
    let (ps, pm, pl): (Option<i32>, Option<i32>, Option<i32>) = (row.get(8), row.get(9), row.get(10));
    StationRef {
        station_id: row.get::<i64, _>(0),
        station: row.get::<Option<String>, _>(1).unwrap_or_default(),
        system: row.get::<String, _>(2),
        system_id64: row.get::<i64, _>(3),
        x: row.get::<Option<f64>, _>(4).unwrap_or(0.0),
        y: row.get::<Option<f64>, _>(5).unwrap_or(0.0),
        z: row.get::<Option<f64>, _>(6).unwrap_or(0.0),
        arrival_ls: row.get::<Option<f64>, _>(7),
        max_pad: PadSize::from_counts(pl.map(i64::from), pm.map(i64::from), ps.map(i64::from)),
        class: StationClass::of(row.get::<Option<String>, _>(12).as_deref()),
        is_carrier: row.get::<bool, _>(11),
        controlling_power: row.get::<Option<String>, _>(13),
        power_state: row.get::<Option<String>, _>(14),
        powers: row
            .get::<Option<String>, _>(15)
            .map(|s| s.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect())
            .unwrap_or_default(),
    }
}

/// The local finder's rules, in its order: carriers, arrival distance,
/// pad, then the nearest `max_stations`.
pub(crate) fn exclude(
    all: Vec<StationRef>,
    origin: (f64, f64, f64),
    c: &Constraints,
    excluded: &mut Excluded,
) -> Vec<StationRef> {
    let mut kept: Vec<StationRef> = Vec::with_capacity(all.len());
    for s in all {
        if s.is_carrier && !c.include_carriers {
            excluded.carriers += 1;
            continue;
        }
        if s.arrival_ls.unwrap_or(0.0) > c.max_arrival_ls {
            excluded.too_far_from_star += 1;
            continue;
        }
        if let Some(required) = c.min_pad {
            match s.max_pad {
                None => {
                    excluded.pad_unknown += 1;
                    continue;
                }
                Some(pad) if !pad.fits(required) => {
                    excluded.pad_too_small += 1;
                    continue;
                }
                Some(_) => {}
            }
        }
        kept.push(s);
    }
    let cap = c.max_stations.max(1);
    if kept.len() > cap {
        let (ox, oy, oz) = origin;
        let d2 = |s: &StationRef| (s.x - ox).powi(2) + (s.y - oy).powi(2) + (s.z - oz).powi(2);
        kept.sort_by(|a, b| d2(a).total_cmp(&d2(b)));
        excluded.beyond_station_cap = kept.len() - cap;
        kept.truncate(cap);
    }
    kept
}

/// Candidate stations, their fresh rows, confiscations applied.
pub async fn prepare(pool: &PgPool, origin: (f64, f64, f64), c: &Constraints) -> Result<Prepared, Refusal> {
    let (ox, oy, oz) = origin;
    let r = c.radius_ly.clamp(1.0, MAX_RADIUS_LY);
    let mut excluded = Excluded::default();
    let mut timing = SearchTiming::default();

    let phase = Instant::now();
    // Freshness gates the candidates BEFORE the distance cap (2026-09-09:
    // Inara's top Wongi route was Röntgen Dock → Metz Enterprise
    // at 37 and 47 ly, and we never showed it). Nearest-2,500-first in
    // the bubble core reaches 33.9 ly from Wongi and fills every slot
    // with stale boards; only 123 of the 7,202 stations with data inside
    // 60 ly had a board under 8 h old, and Inara's route was two of them.
    // A station is a candidate only if it has at least one row inside
    // the price-age window; the cap then rarely binds, and when it does
    // it binds on stations that can actually trade. The EXISTS is one
    // probe of market_station_fresh_idx (station_id, observed_at DESC).
    let rows = sqlx::query(&format!(
        "SELECT {STATION_COLS} \
         FROM stations st JOIN systems sy ON sy.address = st.system_address \
         WHERE sy.cell = ANY($5) \
           AND sy.x BETWEEN $1-$4 AND $1+$4 AND sy.y BETWEEN $2-$4 AND $2+$4 AND sy.z BETWEEN $3-$4 AND $3+$4 \
           AND (sy.x-$1)^2 + (sy.y-$2)^2 + (sy.z-$3)^2 <= $4*$4 \
           AND st.has_market \
           AND EXISTS (SELECT 1 FROM market m WHERE m.station_id = st.id \
                         AND m.observed_at > now() - make_interval(secs => $6))"
    ))
    .bind(ox)
    .bind(oy)
    .bind(oz)
    .bind(r)
    .bind(crate::geo::cells_covering(ox, oy, oz, r))
    .bind(c.max_age_hours.max(0.25) * 3600.0)
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let stations = exclude(rows.iter().map(station_ref).collect(), origin, c, &mut excluded);
    timing.candidates_ms = phase.elapsed().as_millis() as u64;

    let phase = Instant::now();
    let ids: Vec<i64> = stations.iter().map(|s| s.station_id).collect();
    let rows = sqlx::query(
        "SELECT m.station_id, m.commodity_symbol, NULLIF(c.name, ''), m.buy_price, m.sell_price, m.demand, m.supply, \
                EXTRACT(EPOCH FROM now() - m.observed_at)::DOUBLE PRECISION / 3600.0 \
         FROM market m LEFT JOIN commodities c ON c.symbol = m.commodity_symbol \
         WHERE m.station_id = ANY($1) AND m.observed_at > now() - make_interval(secs => $2)",
    )
    .bind(&ids)
    .bind(c.max_age_hours.max(0.25) * 3600.0)
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let mut seen: HashSet<i64> = HashSet::new();
    let mut market: Vec<MarketRow> = rows
        .iter()
        .map(|row| {
            let station_id: i64 = row.get(0);
            seen.insert(station_id);
            MarketRow {
                station_id,
                symbol: row.get::<String, _>(1),
                name: row.get::<Option<String>, _>(2),
                buy_price: row.get::<i64, _>(3),
                sell_price: row.get::<i64, _>(4),
                demand: row.get::<i64, _>(5),
                supply: row.get::<i64, _>(6),
                age_hours: row.get::<f64, _>(7),
            }
        })
        .collect();
    excluded.no_market_data += ids.len() - seen.len();
    timing.market_ms = phase.elapsed().as_millis() as u64;

    let phase = Instant::now();
    let prohibited_rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT station_id, lower(symbol) FROM station_prohibited WHERE station_id = ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await
            .map_err(|e| Refusal::Invalid(e.to_string()))?;
    // Opted in, the sale is allowed exactly where a black market exists
    // to take the goods; elsewhere it stays impossible.
    let black_markets: HashSet<i64> = if c.include_prohibited {
        sqlx::query_scalar::<_, i64>(
            "SELECT station_id FROM station_services WHERE service = 'blackmarket' AND station_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?
        .into_iter()
        .collect()
    } else {
        HashSet::new()
    };
    let mut prohibited: HashMap<i64, HashSet<String>> = HashMap::new();
    for (station_id, symbol) in prohibited_rows {
        if black_markets.contains(&station_id) {
            continue;
        }
        prohibited.entry(station_id).or_default().insert(symbol);
    }
    // `market.commodity_symbol` is the journal's lowercase symbol already;
    // the prohibited list is lowered in SQL to match.
    excluded.confiscated_sales = silence_prohibited(&mut market, &prohibited);
    timing.guards_ms = phase.elapsed().as_millis() as u64;

    Ok(Prepared { stations, rows: market, excluded, timing })
}

// ------------------------------------------------- the docked board

/// The commander's own docked board, sent with a `from_station_id`
/// request (B.4 gap 2 — the one fusion the ship computer keeps:
/// Market.json is fresher than EDDN whenever the commander runs no
/// uploader). Used for this one request only; never stored — the
/// fleet's copy of a board is EDDN's to deliver, and the server keeps
/// nothing it learned from one commander's request.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ClientBoard {
    /// The journal's MarketID; must equal `from_station_id`.
    pub station_id: i64,
    /// Market.json's own `timestamp` (ISO).
    pub observed_at: String,
    pub rows: Vec<ClientBoardRow>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ClientBoardRow {
    /// The journal's lowercase wire symbol (`$gold_name;` → `gold`).
    pub symbol: String,
    #[serde(default)]
    pub buy_price: i64,
    #[serde(default)]
    pub sell_price: i64,
    #[serde(default)]
    pub demand: i64,
    #[serde(default)]
    pub supply: i64,
}

/// A carrier board can exceed this; the client truncates and says so.
pub const MAX_BOARD_ROWS: usize = 400;

/// What the search did with the board, echoed on the report so the
/// Trade tab can say which board it used.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct BoardUse {
    pub used: bool,
    /// `newer` | `absent` (used); `older` | `mismatch` | `invalid_timestamp`
    /// | `unknown_station` (not used).
    pub reason: &'static str,
    /// Rows the request's board contributed.
    pub rows: usize,
}

impl BoardUse {
    fn unused(reason: &'static str) -> Self {
        BoardUse { used: false, reason, rows: 0 }
    }
}

/// The decision and the splice, pure: `stored_newest` is the epoch of
/// the newest stored row at the station (`None` = the server has no
/// board there), `station` the station's row when it is not already a
/// candidate (a stale or absent server board keeps it out of the fresh
/// set; the commander is docked there, so it belongs in). The board's
/// rows replace the stored rows for that station; every other station
/// is untouched.
pub fn fuse_board(
    prepared: &mut Prepared,
    board: &ClientBoard,
    from_station_id: Option<i64>,
    stored_newest: Option<i64>,
    station: Option<StationRef>,
    names: &HashMap<String, String>,
    now: i64,
) -> BoardUse {
    if from_station_id != Some(board.station_id) {
        return BoardUse::unused("mismatch");
    }
    let Some(observed) = ed_domain::freshness::parse_timestamp(&board.observed_at) else {
        return BoardUse::unused("invalid_timestamp");
    };
    if stored_newest.is_some_and(|stored| stored >= observed) {
        return BoardUse::unused("older");
    }
    if !prepared.stations.iter().any(|s| s.station_id == board.station_id) {
        match station {
            Some(s) => prepared.stations.push(s),
            None => return BoardUse::unused("unknown_station"),
        }
    }
    let age_hours = ((now - observed).max(0) as f64) / 3600.0;
    let before = prepared.rows.len();
    prepared.rows.retain(|r| r.station_id != board.station_id);
    if before != prepared.rows.len() {
        // The station had (older) stored rows; it no longer counts as
        // lacking data either way.
    } else {
        prepared.excluded.no_market_data = prepared.excluded.no_market_data.saturating_sub(1);
    }
    let mut rows = 0;
    for r in &board.rows {
        let symbol = r.symbol.trim().to_ascii_lowercase();
        if symbol.is_empty() {
            continue;
        }
        prepared.rows.push(MarketRow {
            station_id: board.station_id,
            name: names.get(&symbol).cloned(),
            symbol,
            buy_price: r.buy_price,
            sell_price: r.sell_price,
            demand: r.demand,
            supply: r.supply,
            age_hours,
        });
        rows += 1;
    }
    BoardUse { used: true, reason: if stored_newest.is_some() { "newer" } else { "absent" }, rows }
}

/// The async half: bounds, the stored board's age, the station's row
/// if it is not a candidate, display names — then [`fuse_board`].
pub async fn fuse(
    pool: &PgPool,
    prepared: &mut Prepared,
    board: &ClientBoard,
    from_station_id: Option<i64>,
) -> Result<BoardUse, Refusal> {
    if board.rows.len() > MAX_BOARD_ROWS {
        return Err(Refusal::Invalid(format!("board carries {} rows; at most {MAX_BOARD_ROWS}", board.rows.len())));
    }
    if from_station_id != Some(board.station_id) {
        return Ok(BoardUse::unused("mismatch"));
    }
    let stored_newest: Option<f64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM max(observed_at))::DOUBLE PRECISION FROM market WHERE station_id = $1",
    )
    .bind(board.station_id)
    .fetch_one(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let station = if prepared.stations.iter().any(|s| s.station_id == board.station_id) {
        None
    } else {
        sqlx::query(&format!(
            "SELECT {STATION_COLS} FROM stations st JOIN systems sy ON sy.address = st.system_address WHERE st.id = $1"
        ))
        .bind(board.station_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?
        .as_ref()
        .map(station_ref)
    };
    let symbols: Vec<String> = board.rows.iter().map(|r| r.symbol.trim().to_ascii_lowercase()).collect();
    let names: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT symbol, name FROM commodities WHERE symbol = ANY($1) AND name <> ''")
            .bind(&symbols)
            .fetch_all(pool)
            .await
            .map_err(|e| Refusal::Invalid(e.to_string()))?
            .into_iter()
            .collect();
    Ok(fuse_board(
        prepared,
        board,
        from_station_id,
        stored_newest.map(|s| s as i64),
        station,
        &names,
        ed_route::profit::now_epoch_secs(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The docked board replaces the station's stored rows when it is
    /// newer, joins the candidate set when the server's board was too
    /// stale to qualify, and is ignored — untouched report — when it is
    /// older, names another station, or carries no parseable time.
    #[test]
    fn the_docked_board_is_used_only_when_newer_and_only_for_its_station() {
        let row = |st: i64, sym: &str, buy: i64, sell: i64| MarketRow {
            station_id: st, symbol: sym.into(), name: None, buy_price: buy, sell_price: sell,
            demand: 10, supply: 10, age_hours: 5.0,
        };
        let fresh = || Prepared {
            stations: vec![station(1, 0.0, Some(PadSize::Large), false, Some(100.0)), station(2, 10.0, Some(PadSize::Large), false, Some(100.0))],
            rows: vec![row(1, "gold", 100, 0), row(2, "gold", 0, 200)],
            excluded: Excluded::default(),
            timing: SearchTiming::default(),
        };
        let board = ClientBoard {
            station_id: 1,
            observed_at: "2026-09-09T10:00:00Z".into(),
            rows: vec![ClientBoardRow { symbol: "Silver".into(), buy_price: 50, sell_price: 0, demand: 0, supply: 900 }],
        };
        let ts = ed_domain::freshness::parse_timestamp("2026-09-09T10:00:00Z").unwrap();
        let names: HashMap<String, String> = [("silver".to_string(), "Silver".to_string())].into_iter().collect();

        // Newer than the stored board: station 1's rows are the board's.
        let mut p = fresh();
        let outcome = fuse_board(&mut p, &board, Some(1), Some(ts - 3_600), None, &names, ts + 1_800);
        assert_eq!(outcome, BoardUse { used: true, reason: "newer", rows: 1 });
        let mine: Vec<&MarketRow> = p.rows.iter().filter(|r| r.station_id == 1).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!((mine[0].symbol.as_str(), mine[0].name.as_deref(), mine[0].supply), ("silver", Some("Silver"), 900));
        assert!((mine[0].age_hours - 0.5).abs() < 1e-9, "age from the board's own timestamp");
        assert_eq!(p.rows.iter().filter(|r| r.station_id == 2).count(), 1, "the other station is untouched");

        // Older than (or equal to) the stored board: nothing changes.
        let mut p = fresh();
        assert_eq!(fuse_board(&mut p, &board, Some(1), Some(ts), None, &names, ts + 60), BoardUse::unused("older"));
        assert_eq!(p.rows.len(), 2);

        // Not the station the request sources from.
        let mut p = fresh();
        assert_eq!(fuse_board(&mut p, &board, Some(2), None, None, &names, ts), BoardUse::unused("mismatch"));
        assert_eq!(fuse_board(&mut p, &board, None, None, None, &names, ts), BoardUse::unused("mismatch"));
        assert_eq!(p.rows.len(), 2);

        // The server has no board there and the station was not a
        // candidate: it joins with the board's rows, or the fusion is
        // refused when the server does not know the station at all.
        let mut p = fresh();
        p.stations.retain(|s| s.station_id != 1);
        p.rows.retain(|r| r.station_id != 1);
        p.excluded.no_market_data = 1;
        let docked = station(1, 0.0, Some(PadSize::Large), false, Some(100.0));
        assert_eq!(fuse_board(&mut p, &board, Some(1), None, Some(docked), &names, ts), BoardUse { used: true, reason: "absent", rows: 1 });
        assert_eq!(p.stations.len(), 2);
        assert_eq!(p.excluded.no_market_data, 0);
        let mut p = fresh();
        p.stations.retain(|s| s.station_id != 1);
        assert_eq!(fuse_board(&mut p, &board, Some(1), None, None, &names, ts), BoardUse::unused("unknown_station"));

        let bad = ClientBoard { observed_at: "yesterday".into(), ..board.clone() };
        let mut p = fresh();
        assert_eq!(fuse_board(&mut p, &bad, Some(1), None, None, &names, ts), BoardUse::unused("invalid_timestamp"));
    }

    fn station(id: i64, x: f64, pad: Option<PadSize>, carrier: bool, arrival: Option<f64>) -> StationRef {
        StationRef {
            station_id: id,
            station: format!("S{id}"),
            system: "Sys".into(),
            system_id64: 1,
            x,
            y: 0.0,
            z: 0.0,
            arrival_ls: arrival,
            max_pad: pad,
            class: StationClass::of(Some("Coriolis")),
            is_carrier: carrier,
            controlling_power: None,
            power_state: None,
            powers: Vec::new(),
        }
    }

    /// Same rules as the local finder's `candidate_stations`: carriers
    /// out unless asked, far-from-star out, pad too small / unknown out
    /// when a pad is required, then the nearest `max_stations` kept.
    #[test]
    fn exclusions_mirror_the_local_finder() {
        let c = Constraints { min_pad: Some(PadSize::Large), max_arrival_ls: 1_000.0, max_stations: 2, ..Constraints::default() };
        let all = vec![
            station(1, 1.0, Some(PadSize::Large), false, Some(10.0)),
            station(2, 2.0, Some(PadSize::Large), true, Some(10.0)),
            station(3, 3.0, Some(PadSize::Medium), false, Some(10.0)),
            station(4, 4.0, None, false, Some(10.0)),
            station(5, 5.0, Some(PadSize::Large), false, Some(5_000.0)),
            station(6, 6.0, Some(PadSize::Large), false, Some(10.0)),
            station(7, 7.0, Some(PadSize::Large), false, Some(10.0)),
        ];
        let mut excluded = Excluded::default();
        let kept = exclude(all, (0.0, 0.0, 0.0), &c, &mut excluded);
        assert_eq!(kept.iter().map(|s| s.station_id).collect::<Vec<_>>(), vec![1, 6]);
        assert_eq!(excluded.carriers, 1);
        assert_eq!(excluded.pad_too_small, 1);
        assert_eq!(excluded.pad_unknown, 1);
        assert_eq!(excluded.too_far_from_star, 1);
        assert_eq!(excluded.beyond_station_cap, 1);
    }
}
