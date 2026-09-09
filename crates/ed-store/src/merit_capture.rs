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
pub fn capture(conn: &Connection, seen: &mut HashSet<String>) -> usize {
    let placeholders = EARNING_EVENTS
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT event, raw FROM events WHERE event IN ({placeholders}) ORDER BY file, offset"
    );

    let mut candidates: Vec<Candidate> = Vec::new();
    {
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return 0;
        };
        let rows = stmt.query_map(rusqlite::params_from_iter(EARNING_EVENTS.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        let Ok(rows) = rows else { return 0 };
        for (event, raw) in rows.flatten() {
            let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if let Some(c) = candidate_from(&event, &v, None) {
                candidates.push(c);
            }
        }
    }

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

        tracing::info!(
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
