//! Whose companion file is this? The game keeps one Status.json, one
//! Cargo.json and so on per Windows user, with no FID inside, and two
//! instances overwrite the same files. Timing cannot tell them apart
//! (15.6% of state lines ambiguous at 3 s on the donor data,
//! `docs/benches/2026-10-24-two-account-folder-baseline.csv`); content
//! can. Each rule compares the file with what the store's own journal
//! last said.
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension as _};
use serde_json::Value;

/// How well a companion file fits one commander's store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Score {
    /// The file says what this store's journal says.
    Exact,
    /// Status.json only: how far its Balance is from this store's credits.
    Distance(i64),
    /// The file contradicts this store's journal.
    Mismatch,
    /// This store cannot say (no journal fact to compare, or the file
    /// carries nothing to compare).
    Unknown,
}

fn latest_raw(conn: &Connection, event: &str, having: Option<&str>) -> Result<Option<Value>> {
    let sql = match having {
        Some(field) => format!("SELECT raw FROM events WHERE event = ?1 AND json_extract(raw, '{field}') IS NOT NULL ORDER BY ts DESC, file DESC, offset DESC LIMIT 1"),
        None => "SELECT raw FROM events WHERE event = ?1 ORDER BY ts DESC, file DESC, offset DESC LIMIT 1".to_string(),
    };
    let raw: Option<String> = conn.query_row(&sql, [event], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

fn item_total(v: &Value) -> Option<i64> {
    let mut total = 0;
    let mut any = false;
    for key in ["Items", "Components", "Consumables", "Data"] {
        if let Some(arr) = v.get(key).and_then(Value::as_array) {
            any = true;
            total += arr.iter().filter_map(|i| i.get("Count").and_then(Value::as_i64)).sum::<i64>();
        }
    }
    any.then_some(total)
}

fn module_set(v: &Value) -> Option<std::collections::BTreeSet<(String, String)>> {
    let arr = v.get("Modules")?.as_array()?;
    Some(arr.iter().filter_map(|m| Some((m.get("Slot")?.as_str()?.to_ascii_lowercase(), m.get("Item")?.as_str()?.to_ascii_lowercase()))).collect())
}

fn eq_or_mismatch(same: bool) -> Score {
    if same { Score::Exact } else { Score::Mismatch }
}

/// Score `v` (the parsed companion file `name`) against this store.
pub fn score(conn: &Connection, name: &str, v: &Value) -> Result<Score> {
    Ok(match name {
        "Status.json" => {
            let (Some(balance), Some(est)) = (v.get("Balance").and_then(Value::as_i64), crate::session::credits_estimate(conn)?) else {
                return Ok(Score::Unknown);
            };
            Score::Distance((balance - est).abs())
        }
        "Cargo.json" => {
            let Some(last) = latest_raw(conn, "Cargo", Some("$.Count"))? else { return Ok(Score::Unknown) };
            let Some(count) = v.get("Count").and_then(Value::as_i64) else { return Ok(Score::Unknown) };
            let vessel = v.get("Vessel").and_then(Value::as_str).unwrap_or("Ship");
            eq_or_mismatch(last.get("Count").and_then(Value::as_i64) == Some(count) && last.get("Vessel").and_then(Value::as_str).unwrap_or("Ship") == vessel)
        }
        "Market.json" | "Outfitting.json" | "Shipyard.json" => {
            let Some(id) = v.get("MarketID").and_then(Value::as_i64) else { return Ok(Score::Unknown) };
            let docked: Option<i64> = conn
                .query_row("SELECT market_id FROM events WHERE event = 'Docked' AND market_id IS NOT NULL ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
                .optional()?;
            match docked {
                None => Score::Unknown,
                Some(d) => eq_or_mismatch(d == id),
            }
        }
        "NavRoute.json" => {
            let Some(first) = v.get("Route").and_then(Value::as_array).and_then(|r| r.first()) else { return Ok(Score::Unknown) };
            let Some(start) = first.get("SystemAddress").and_then(Value::as_i64) else { return Ok(Score::Unknown) };
            let here: Option<i64> = conn
                .query_row("SELECT system_address FROM events WHERE event IN ('FSDJump','Location','CarrierJump') AND system_address IS NOT NULL ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
                .optional()?;
            match here {
                None => Score::Unknown,
                Some(h) => eq_or_mismatch(h == start),
            }
        }
        "ShipLocker.json" | "Backpack.json" => {
            let event = name.trim_end_matches(".json");
            let Some(last) = latest_raw(conn, event, Some("$.Items"))? else { return Ok(Score::Unknown) };
            match (item_total(v), item_total(&last)) {
                (Some(a), Some(b)) => eq_or_mismatch(a == b),
                _ => Score::Unknown,
            }
        }
        "ModulesInfo.json" => {
            let Some(last) = latest_raw(conn, "Loadout", Some("$.Modules"))? else { return Ok(Score::Unknown) };
            match (module_set(v), module_set(&last)) {
                (Some(a), Some(b)) => eq_or_mismatch(a == b),
                _ => Score::Unknown,
            }
        }
        _ => Score::Unknown,
    })
}

/// Which stores receive the file. One store gets everything (a single
/// commander is today's app). Otherwise every `Exact` store does; failing
/// that, the clearly nearest Balance (the runner-up at least twice as far).
/// Nobody means unknown: no store takes it and each is marked so.
pub fn choose(scores: &[(String, Score)]) -> Vec<String> {
    if scores.len() == 1 {
        return vec![scores[0].0.clone()];
    }
    let exact: Vec<String> = scores.iter().filter(|(_, s)| *s == Score::Exact).map(|(f, _)| f.clone()).collect();
    if !exact.is_empty() {
        return exact;
    }
    let mut dists: Vec<(i64, &String)> = scores.iter().filter_map(|(f, s)| if let Score::Distance(d) = s { Some((*d, f)) } else { None }).collect();
    dists.sort();
    match dists.as_slice() {
        [] => Vec::new(),
        [(_, only)] => vec![(*only).clone()],
        [(best, f), (next, _), ..] if *next >= best.saturating_mul(2) => vec![(*f).clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use serde_json::json;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }
    fn ev(conn: &Connection, ts: &str, offset: i64, v: serde_json::Value) {
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, system_address, market_id, raw) VALUES ('J.log', ?1, ?2, ?3, ?4, ?5, ?6)",
            params![offset, ts, v["event"].as_str().unwrap(), v.get("SystemAddress").and_then(|x| x.as_i64()), v.get("MarketID").and_then(|x| x.as_i64()), v.to_string()],
        ).unwrap();
    }

    #[test]
    fn credits_estimate_is_the_last_loadgame_plus_the_money_since() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"LoadGame","FID":"F1","Credits":1_000_000}));
        ev(&conn, "2026-10-17T18:05:00Z", 1, json!({"event":"MissionCompleted","Reward":250_000}));
        ev(&conn, "2026-10-17T18:06:00Z", 2, json!({"event":"MarketBuy","TotalCost":100_000}));
        ev(&conn, "2026-10-17T18:07:00Z", 3, json!({"event":"Bounty","TotalReward":999_999})); // a voucher, not paid yet
        ev(&conn, "2026-10-17T18:08:00Z", 4, json!({"event":"RedeemVoucher","Amount":50_000}));
        assert_eq!(crate::session::credits_estimate(&conn).unwrap(), Some(1_200_000));
        let empty = mem();
        assert_eq!(crate::session::credits_estimate(&empty).unwrap(), None);
    }

    #[test]
    fn status_scores_by_balance_distance() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"LoadGame","FID":"F1","Credits":5_000_000}));
        assert_eq!(score(&conn, "Status.json", &json!({"Balance": 5_010_000})).unwrap(), Score::Distance(10_000));
        assert_eq!(score(&conn, "Status.json", &json!({"Flags": 0})).unwrap(), Score::Unknown, "no Balance (on foot menus) is unknown");
        assert_eq!(score(&mem(), "Status.json", &json!({"Balance": 1})).unwrap(), Score::Unknown, "no LoadGame yet is unknown");
    }

    #[test]
    fn cargo_scores_by_count_and_vessel() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"Cargo","Vessel":"Ship","Count":64}));
        assert_eq!(score(&conn, "Cargo.json", &json!({"Vessel":"Ship","Count":64,"Inventory":[]})).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "Cargo.json", &json!({"Vessel":"Ship","Count":0,"Inventory":[]})).unwrap(), Score::Mismatch);
        assert_eq!(score(&mem(), "Cargo.json", &json!({"Vessel":"Ship","Count":0})).unwrap(), Score::Unknown);
    }

    #[test]
    fn market_outfitting_shipyard_score_by_market_id() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"Docked","MarketID":3_700_000_001i64,"StationName":"X"}));
        for name in ["Market.json", "Outfitting.json", "Shipyard.json"] {
            assert_eq!(score(&conn, name, &json!({"MarketID": 3_700_000_001i64})).unwrap(), Score::Exact, "{name}");
            assert_eq!(score(&conn, name, &json!({"MarketID": 3_700_000_002i64})).unwrap(), Score::Mismatch, "{name}");
        }
        assert_eq!(score(&mem(), "Market.json", &json!({"MarketID": 1})).unwrap(), Score::Unknown);
    }

    #[test]
    fn navroute_scores_by_its_first_hop_against_the_current_system() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"FSDJump","StarSystem":"A","SystemAddress":11}));
        let here = json!({"Route":[{"StarSystem":"A","SystemAddress":11},{"StarSystem":"B","SystemAddress":12}]});
        let elsewhere = json!({"Route":[{"StarSystem":"C","SystemAddress":13}]});
        assert_eq!(score(&conn, "NavRoute.json", &here).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "NavRoute.json", &elsewhere).unwrap(), Score::Mismatch);
        assert_eq!(score(&conn, "NavRoute.json", &json!({"Route":[]})).unwrap(), Score::Unknown, "a cleared route names nobody");
    }

    #[test]
    fn shiplocker_and_backpack_score_by_item_totals() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"ShipLocker","Items":[{"Name":"a","Count":2}],"Components":[{"Name":"b","Count":3}],"Consumables":[],"Data":[{"Name":"c","Count":1}]}));
        ev(&conn, "2026-10-17T18:01:00Z", 1, json!({"event":"ShipLocker"})); // the empty pointer the game writes later
        assert_eq!(score(&conn, "ShipLocker.json", &json!({"Items":[{"Name":"a","Count":2}],"Components":[{"Name":"b","Count":3}],"Consumables":[],"Data":[{"Name":"c","Count":1}]})).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "ShipLocker.json", &json!({"Items":[],"Components":[],"Consumables":[],"Data":[]})).unwrap(), Score::Mismatch);
        assert_eq!(score(&mem(), "Backpack.json", &json!({"Items":[]})).unwrap(), Score::Unknown);
    }

    #[test]
    fn modulesinfo_scores_by_the_module_set_of_the_latest_loadout() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"Loadout","Ship":"mandalay","ShipID":31,"Modules":[{"Slot":"PowerPlant","Item":"int_powerplant_size5_class5"},{"Slot":"FrameShiftDrive","Item":"int_hyperdrive_overcharge_size5_class5"}]}));
        assert_eq!(score(&conn, "ModulesInfo.json", &json!({"Modules":[{"Slot":"FrameShiftDrive","Item":"int_hyperdrive_overcharge_size5_class5"},{"Slot":"PowerPlant","Item":"int_powerplant_size5_class5"}]})).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "ModulesInfo.json", &json!({"Modules":[{"Slot":"PowerPlant","Item":"int_powerplant_size3_class1"}]})).unwrap(), Score::Mismatch);
    }

    #[test]
    fn an_unlisted_companion_is_unknown() {
        assert_eq!(score(&mem(), "FCMaterials.json", &json!({})).unwrap(), Score::Unknown);
    }

    #[test]
    fn choose_gives_a_single_store_everything() {
        assert_eq!(choose(&[("F1".to_string(), Score::Unknown)]), vec!["F1".to_string()]);
        assert_eq!(choose(&[("F1".to_string(), Score::Mismatch)]), vec!["F1".to_string()]);
    }

    #[test]
    fn choose_prefers_exact_then_a_clear_nearest_balance() {
        let f = |s: &str| s.to_string();
        assert_eq!(choose(&[(f("F1"), Score::Exact), (f("F2"), Score::Mismatch)]), vec![f("F1")]);
        assert_eq!(choose(&[(f("F1"), Score::Exact), (f("F2"), Score::Exact)]), vec![f("F1"), f("F2")], "a tie with equal content goes to both");
        assert_eq!(choose(&[(f("F1"), Score::Distance(1_000)), (f("F2"), Score::Distance(2_000_000_000))]), vec![f("F1")]);
        assert_eq!(choose(&[(f("F1"), Score::Distance(1_000)), (f("F2"), Score::Distance(1_500))]), Vec::<String>::new(), "too close to call is unknown");
        assert_eq!(choose(&[(f("F1"), Score::Distance(1_000)), (f("F2"), Score::Unknown)]), vec![f("F1")]);
        assert_eq!(choose(&[(f("F1"), Score::Mismatch), (f("F2"), Score::Unknown)]), Vec::<String>::new());
    }
}
