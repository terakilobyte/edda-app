//! Attribute Powerplay merit awards to whatever earned them.
//!
//! `PowerplayMerits` says only how many merits arrived, never why. Everything
//! here is inference, and the inference is only as good as its discipline.
//!
//! # Why the obvious approach fails
//!
//! The first version matched each award to the nearest preceding *kill*.
//! That works when kills are ninety seconds apart and collapses in a
//! resource extraction site: a 600 cr bounty appeared to earn 176 merits,
//! because scan awards landing between kills were charged to whichever kill
//! preceded them.
//!
//! There are at least four merit sources, and they interleave:
//!
//! | Source | Trigger | Scaling |
//! |---|---|---|
//! | Trade | `MarketSell` | by profit |
//! | Combat | `Bounty`, `FactionKillBond`, `CapShipBond`, `PVPKill` | by reward |
//! | Scan | `ShipTargeted` with `ScanStage == 3` | flat (+7 / +8 / +14 observed) |
//! | Delivery | `PowerplayDeliver` | very high per credit |
//!
//! So attribution walks a merged timeline of every merit-worthy event and
//! assigns each award to the one immediately before it. An award with no
//! candidate in range is recorded unattributed rather than forced onto the
//! nearest plausible row.
//!
//! # Trust
//!
//! `implied_k` is only emitted when the source is credit-scaled *and* no
//! other candidate sits close enough to have been the real cause. Everything
//! else records the facts and withholds the inference, so a fit over this
//! dataset is not quietly poisoned.

use crate::observe::{record, Observation};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashSet;

/// How long after an event an award may arrive and still be attributed to it.
const WINDOW_SECS: i64 = 5;

/// Events that can earn merits, in the order the journal wrote them.
const EARNING_EVENTS: &[&str] = &[
    "MarketSell",
    "Bounty",
    "FactionKillBond",
    "CapShipBond",
    "PVPKill",
    "ShipTargeted",
    "PowerplayDeliver",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Sale,
    Kill,
    Scan,
    Delivery,
}

impl Source {
    /// Whether merits from this source scale with credits earned.
    pub fn is_credit_scaled(&self) -> bool {
        matches!(self, Source::Sale | Source::Kill | Source::Delivery)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Sale => "sale",
            Source::Kill => "kill",
            Source::Scan => "scan",
            Source::Delivery => "delivery",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub ts: String,
    pub source: Source,
    pub label: Option<String>,
    pub detail: Option<String>,
    /// Credits earned, where the source has any.
    pub credits: Option<i64>,
    pub system: Option<String>,
}

/// Turn one journal event into a merit candidate, if it can earn merits.
///
/// A `ShipTargeted` only qualifies at `ScanStage == 3`; the earlier stages
/// fire constantly while a target is locked and would swamp the timeline.
fn candidate_from(event: &str, v: &Value, system: Option<String>) -> Option<Candidate> {
    let ts = v.get("timestamp")?.as_str()?.to_string();
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let i = |k: &str| v.get(k).and_then(Value::as_i64);

    let c = match event {
        "MarketSell" => {
            let count = i("Count").unwrap_or(0);
            let total = i("TotalSale");
            let paid = i("AvgPricePaid").unwrap_or(0);
            Candidate {
                ts,
                source: Source::Sale,
                label: s("Type_Localised").or_else(|| s("Type")),
                detail: Some(format!("{count} t")),
                credits: total.map(|t| t - paid * count),
                system,
            }
        }
        "Bounty" | "FactionKillBond" | "CapShipBond" | "PVPKill" => Candidate {
            ts,
            source: Source::Kill,
            label: s("Target")
                .map(|t| ed_journal::ships::display_name_or(&t, s("Target_Localised").as_deref())),
            detail: s("PilotName_Localised").or_else(|| s("PilotName")),
            credits: i("TotalReward").or_else(|| i("Reward")),
            system,
        },
        "ShipTargeted" => {
            if i("ScanStage") != Some(3) {
                return None;
            }
            Candidate {
                ts,
                source: Source::Scan,
                label: s("Ship").map(|t| {
                    ed_journal::ships::display_name_or(&t, s("Ship_Localised").as_deref())
                }),
                detail: s("PilotName_Localised").or_else(|| s("PilotName")),
                credits: None,
                system,
            }
        }
        "PowerplayDeliver" => Candidate {
            ts,
            source: Source::Delivery,
            label: s("Type_Localised").or_else(|| s("Type")),
            detail: i("Count").map(|c| format!("{c} units")),
            credits: None,
            system,
        },
        _ => return None,
    };
    Some(c)
}

/// Attribute every merit award and append newly-seen ones to the journal.
///
/// Idempotent: `seen` keys each award by timestamp and amount, so repeated
/// syncs over a live session never duplicate a row.
/// What capture carries from one sync to the next.
///
/// Until 2026-09-17 every sync re-read and re-parsed every earning
/// event in the store — no bound, `seen` consulted only after the parse
/// — and the watcher syncs at least every five seconds. On a seven-year
/// journal that was 5.1 s of JSON parsing per wake, so the process
/// finished each pass just as the next was due and pinned a core for
/// as long as the app ran: the "burning down his CPU" report on 0.3.4.
/// The bench that measured it is docs/benches/2026-09-16-first-sync-
/// veteran-journal.csv; it was on the roadmap when the release shipped.
///
/// Now the scan resumes from the last earning event it read, by time
/// then file then offset — the same shape as the derive bookmark fixed
/// in 0.3.4 — and the candidates it has already parsed are kept, pruned
/// to the window an award could still reach. The first capture in a
/// process is still a full scan; the ones after it read only what is new.
pub struct CaptureState {
    /// Award keys already written to the observation journal. Sync runs
    /// on every journal write, so without this one award would be
    /// recorded dozens of times over a session.
    pub seen: HashSet<String>,
    candidates: Vec<Candidate>,
    /// `(ts, file, offset)` of the last earning event scanned.
    mark: Option<(String, String, i64)>,
    /// Earning events read by the most recent capture. On a quiet sync
    /// this is the number that must be zero.
    pub last_scanned: usize,
}

impl CaptureState {
    pub fn new(seen: HashSet<String>) -> Self {
        CaptureState { seen, candidates: Vec::new(), mark: None, last_scanned: 0 }
    }
}

#[cfg(test)]
impl CaptureState {
    fn retained(&self) -> usize {
        self.candidates.len()
    }
}

impl Default for CaptureState {
    fn default() -> Self {
        Self::new(HashSet::new())
    }
}

pub fn capture(conn: &Connection, state: &mut CaptureState) -> usize {
    let placeholders = EARNING_EVENTS
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    // Numbered placeholders after the event list: 7 events, then the mark.
    let n = EARNING_EVENTS.len();
    let range = match state.mark {
        Some(_) => format!(
            " AND (ts > ?{t} OR (ts = ?{t} AND (file > ?{f} OR (file = ?{f} AND offset > ?{o}))))",
            t = n + 1,
            f = n + 2,
            o = n + 3
        ),
        None => String::new(),
    };
    let sql = format!(
        "SELECT event, raw, ts, file, offset FROM events WHERE event IN ({placeholders}){range} \
         ORDER BY ts, file, offset"
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> =
        EARNING_EVENTS.iter().map(|e| Box::new(*e) as Box<dyn rusqlite::ToSql>).collect();
    if let Some((ts, file, offset)) = &state.mark {
        params.push(Box::new(ts.clone()));
        params.push(Box::new(file.clone()));
        params.push(Box::new(*offset));
    }

    state.last_scanned = 0;
    {
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return 0;
        };
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        });
        let Ok(rows) = rows else { return 0 };
        for (event, raw, ts, file, offset) in rows.flatten() {
            state.last_scanned += 1;
            state.mark = Some((ts, file, offset));
            let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if let Some(c) = candidate_from(&event, &v, None) {
                state.candidates.push(c);
            }
        }
    }
    tracing::trace!(scanned = state.last_scanned, retained = state.candidates.len(), "merit capture scan");
    let candidates = &state.candidates;
    let seen = &mut state.seen;

    let awards: Vec<(String, i64, Option<String>)> = {
        let mut stmt = match conn
            .prepare("SELECT ts, COALESCE(merits_gained,0), power FROM merit_events ORDER BY ts")
        {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let collected: Vec<_> = match stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        };
        collected
    };

    let mut recorded = 0;
    for (award_ts, merits, power) in awards {
        if merits <= 0 {
            continue;
        }
        let key = format!("award|{award_ts}|{merits}");
        if !seen.insert(key) {
            continue;
        }

        // Candidates at or before the award, inside the window.
        let in_range: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| {
                crate::query::seconds_between_ts(&c.ts, &award_ts)
                    .is_some_and(|d| (0..=WINDOW_SECS).contains(&d))
            })
            .collect();

        let Some(best) = in_range.last().copied() else {
            record(&Observation::MeritUnattributed {
                ts: award_ts,
                merits,
                power,
                preceding: Vec::new(),
            });
            recorded += 1;
            continue;
        };

        // Only one candidate in range means the attribution is unambiguous.
        let unambiguous = in_range.len() == 1;
        let implied_k = match (unambiguous, best.source.is_credit_scaled(), best.credits) {
            (true, true, Some(c)) if c > 0 => Some(c as f64 / merits as f64),
            _ => None,
        };

        // Trace, not info (maintainer, 2026-09-16: "noisy as all hell"):
        // this fires once per merit award, and a first sync over a long
        // journal attributes thousands of them in one pass. Same rule as
        // the per-sync line in Store::sync -- RUST_LOG=ed_store=trace
        // brings it back when the attribution itself is in question.
        tracing::trace!(
            source = best.source.as_str(),
            label = best.label.as_deref().unwrap_or("?"),
            credits = best.credits.unwrap_or(0),
            merits,
            implied_k = implied_k.unwrap_or(f64::NAN),
            unambiguous,
            competing = in_range.len(),
            "merit award attributed"
        );

        record(&Observation::MeritAward {
            ts: award_ts,
            source: best.source.as_str().to_string(),
            label: best.label.clone(),
            detail: best.detail.clone(),
            credits: best.credits,
            merits,
            power,
            system: best.system.clone(),
            competing_candidates: in_range.len(),
            implied_k,
        });
        recorded += 1;
    }

    // Keep only candidates an award could still reach. Awards arrive in
    // time order (events are applied by timestamp since 0.3.4), so
    // anything older than the newest award's window is done with.
    let newest_award: Option<String> = conn
        .query_row("SELECT MAX(ts) FROM merit_events", [], |r| r.get(0))
        .ok()
        .flatten();
    if let Some(newest) = newest_award {
        state.candidates.retain(|c| {
            crate::query::seconds_between_ts(&c.ts, &newest).is_none_or(|d| d <= WINDOW_SECS)
        });
    }

    recorded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(event: &str, extra: Value) -> Value {
        let mut v = json!({ "timestamp": "2026-08-26T03:00:00Z", "event": event });
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    }

    #[test]
    fn a_partial_scan_is_not_a_candidate() {
        // Stages 0-2 fire constantly while a target is locked; only a
        // completed scan earns anything, and counting the rest would swamp
        // the timeline and steal awards from real kills.
        for stage in [0, 1, 2] {
            let v = ev(
                "ShipTargeted",
                json!({ "ScanStage": stage, "Ship": "eagle" }),
            );
            assert!(
                candidate_from("ShipTargeted", &v, None).is_none(),
                "stage {stage}"
            );
        }
        let v = ev("ShipTargeted", json!({ "ScanStage": 3, "Ship": "eagle" }));
        let c = candidate_from("ShipTargeted", &v, None).expect("stage 3 is a scan");
        assert_eq!(c.source, Source::Scan);
        assert_eq!(
            c.credits, None,
            "a scan earns a flat award, not a scaled one"
        );
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    fn sale(conn: &Connection, offset: i64, ts: &str) {
        conn.execute(
            "INSERT INTO events (file,offset,ts,event,raw) VALUES ('J.log',?1,?2,'MarketSell',?3)",
            rusqlite::params![
                offset,
                ts,
                format!(r#"{{"timestamp":"{ts}","event":"MarketSell","Type":"gold","Count":10,"TotalSale":1000,"AvgPricePaid":0}}"#)
            ],
        )
        .unwrap();
    }

    fn award(conn: &Connection, offset: i64, ts: &str) {
        conn.execute(
            "INSERT INTO merit_events (file,offset,ts,power,merits_gained,total_merits) VALUES ('J.log',?1,?2,'Aisling Duval',7,7)",
            rusqlite::params![offset, ts],
        )
        .unwrap();
    }

    /// The defect: every sync re-read and re-parsed every earning event in
    /// the store, so a quiet sync on a seven-year journal cost the whole
    /// scan again. A second capture over an unchanged store must read
    /// nothing at all.
    #[test]
    fn a_quiet_capture_reads_no_events() {
        let conn = db();
        sale(&conn, 0, "2026-08-25T10:00:00Z");
        award(&conn, 1, "2026-08-25T10:00:01Z");
        let mut state = CaptureState::default();
        assert_eq!(capture(&conn, &mut state), 1);
        assert_eq!(state.last_scanned, 1, "the first capture reads the store");
        assert_eq!(capture(&conn, &mut state), 0);
        assert_eq!(state.last_scanned, 0, "nothing changed, so nothing is read");
    }

    /// Only what arrived after the mark is read, and it is read.
    #[test]
    fn a_new_earning_event_after_the_mark_is_read() {
        let conn = db();
        sale(&conn, 0, "2026-08-25T10:00:00Z");
        award(&conn, 1, "2026-08-25T10:00:01Z");
        let mut state = CaptureState::default();
        capture(&conn, &mut state);
        sale(&conn, 2, "2026-08-25T11:00:00Z");
        award(&conn, 3, "2026-08-25T11:00:01Z");
        assert_eq!(capture(&conn, &mut state), 1, "the new award is attributed");
        assert_eq!(state.last_scanned, 1, "one new event, not a rescan of two");
    }

    /// A sale scanned in one sync must still be there when its award lands
    /// in the next: the retained tail is consulted, not a fresh scan.
    #[test]
    fn a_retained_candidate_serves_an_award_that_arrives_later() {
        let conn = db();
        sale(&conn, 0, "2026-08-25T10:00:00Z");
        let mut state = CaptureState::default();
        assert_eq!(capture(&conn, &mut state), 0, "no award yet");
        assert_eq!(state.retained(), 1, "the sale is kept for the award to come");
        award(&conn, 1, "2026-08-25T10:00:01Z");
        assert_eq!(capture(&conn, &mut state), 1);
        assert_eq!(state.last_scanned, 0, "served from the tail; no rescan");
    }

    /// Candidates an award can no longer reach are let go, so the tail
    /// does not grow for the life of the process.
    #[test]
    fn the_tail_is_pruned_to_the_window() {
        let conn = db();
        sale(&conn, 0, "2026-08-25T10:00:00Z");
        award(&conn, 1, "2026-08-25T10:00:30Z");
        let mut state = CaptureState::default();
        capture(&conn, &mut state);
        assert_eq!(state.retained(), 0, "30 s before the newest award is outside the 5 s window");
    }

    #[test]
    fn a_sale_reports_profit_not_gross() {
        // Merits track profit; using TotalSale for a bought commodity would
        // inflate the implied constant by the purchase price.
        let v = ev(
            "MarketSell",
            json!({ "Type": "cryolite", "Count": 1008, "TotalSale": 29_661_408, "AvgPricePaid": 10_874 }),
        );
        let c = candidate_from("MarketSell", &v, None).unwrap();
        assert_eq!(c.credits, Some(29_661_408 - 10_874 * 1008));
    }

    #[test]
    fn a_mined_sale_has_no_purchase_price_so_gross_is_profit() {
        let v = ev(
            "MarketSell",
            json!({ "Type": "monazite", "Count": 5, "TotalSale": 969_355, "AvgPricePaid": 0 }),
        );
        assert_eq!(
            candidate_from("MarketSell", &v, None).unwrap().credits,
            Some(969_355)
        );
    }

    #[test]
    fn a_bounty_uses_total_reward_across_factions() {
        let v = ev(
            "Bounty",
            json!({ "Target": "anaconda", "TotalReward": 1_621_122,
                    "Rewards": [{"Faction":"A","Reward":800_000},{"Faction":"B","Reward":821_122}] }),
        );
        let c = candidate_from("Bounty", &v, None).unwrap();
        assert_eq!(c.source, Source::Kill);
        assert_eq!(c.credits, Some(1_621_122));
    }

    #[test]
    fn only_credit_scaled_sources_can_yield_a_constant() {
        assert!(Source::Sale.is_credit_scaled());
        assert!(Source::Kill.is_credit_scaled());
        assert!(Source::Delivery.is_credit_scaled());
        // A flat scan award divided by credits would be meaningless.
        assert!(!Source::Scan.is_credit_scaled());
    }
}
