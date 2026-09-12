//! Reconstructs current material counts from a run of journal lines.
//!
//! The journal only writes a *full* `Materials` snapshot occasionally (game
//! load, sometimes on dock). Everything after that is deltas:
//! `MaterialCollected`, `MaterialDiscarded`, `MaterialTrade`,
//! `EngineerCraft` (consumes ingredients), `EngineerContribution` (donating
//! materials to unlock an engineer), and `Synthesis` (consumes materials).
//! Reading only the last snapshot -- which is what a quick manual grep does
//! -- silently misses everything collected or traded since. This module
//! replays the whole sequence instead, in order, always keyed by the
//! internal symbol so it stays exact.

use serde_json::Value;
use std::collections::HashMap;

/// symbol (lowercase) -> current count. Zero/negative counts are dropped.
pub type Inventory = HashMap<String, i64>;

fn get_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn get_i64(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Apply one journal event (already parsed) to the running inventory.
/// Unrecognized events are ignored. This is the single place that encodes
/// "which events change material counts and how" -- keep it exhaustive
/// rather than adding ad-hoc greps elsewhere.
pub fn apply_event(inventory: &mut Inventory, event: &Value) {
    let Some(etype) = get_str(event, "event") else {
        return;
    };

    match etype {
        "Materials" => {
            inventory.clear();
            for cat in ["Raw", "Manufactured", "Encoded"] {
                if let Some(entries) = event.get(cat).and_then(Value::as_array) {
                    for entry in entries {
                        if let Some(name) = get_str(entry, "Name") {
                            inventory.insert(name.to_lowercase(), get_i64(entry, "Count"));
                        }
                    }
                }
            }
        }

        "MaterialCollected" => {
            if let Some(name) = get_str(event, "Name") {
                let count = if event.get("Count").is_some() {
                    get_i64(event, "Count")
                } else {
                    1
                };
                *inventory.entry(name.to_lowercase()).or_insert(0) += count;
            }
        }

        "MaterialDiscarded" => {
            if let Some(name) = get_str(event, "Name") {
                let count = if event.get("Count").is_some() {
                    get_i64(event, "Count")
                } else {
                    1
                };
                let entry = inventory.entry(name.to_lowercase()).or_insert(0);
                *entry = (*entry - count).max(0);
            }
        }

        "MaterialTrade" => {
            if let Some(paid) = event.get("Paid") {
                if let Some(name) = get_str(paid, "Material") {
                    let entry = inventory.entry(name.to_lowercase()).or_insert(0);
                    *entry = (*entry - get_i64(paid, "Quantity")).max(0);
                }
            }
            if let Some(received) = event.get("Received") {
                if let Some(name) = get_str(received, "Material") {
                    *inventory.entry(name.to_lowercase()).or_insert(0) +=
                        get_i64(received, "Quantity");
                }
            }
        }

        "EngineerCraft" => {
            if let Some(ingredients) = event.get("Ingredients").and_then(Value::as_array) {
                for ing in ingredients {
                    if let Some(name) = get_str(ing, "Name") {
                        let entry = inventory.entry(name.to_lowercase()).or_insert(0);
                        *entry = (*entry - get_i64(ing, "Count")).max(0);
                    }
                }
            }
        }

        "EngineerContribution" if get_str(event, "Type") == Some("Materials") => {
            if let Some(name) = get_str(event, "Material") {
                let entry = inventory.entry(name.to_lowercase()).or_insert(0);
                *entry = (*entry - get_i64(event, "Quantity")).max(0);
            }
        }

        "Synthesis" => {
            if let Some(materials) = event.get("Materials").and_then(Value::as_array) {
                for mat in materials {
                    if let Some(name) = get_str(mat, "Name") {
                        let entry = inventory.entry(name.to_lowercase()).or_insert(0);
                        *entry = (*entry - get_i64(mat, "Count")).max(0);
                    }
                }
            }
        }

        _ => {}
    }
}

/// Replay a full run of raw journal lines (one JSON object per line, exactly
/// as the game writes them -- blank lines and non-JSON lines are skipped).
pub fn replay_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Inventory {
    let mut inventory = Inventory::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            apply_event(&mut inventory, &event);
        }
    }
    inventory.retain(|_, count| *count > 0);
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real lines captured from an actual session, used as a regression fixture
    // for exactly the propulsion-elements / eccentric-hyperspace-trajectories
    // undercounts that happened when this was done by hand with grep.
    const SAMPLE: &str = r#"
{ "timestamp":"2026-08-23T19:55:50Z", "event":"Materials", "Raw":[ { "Name":"tellurium", "Count":48 } ], "Manufactured":[ { "Name":"chemicalprocessors", "Count":13 }, { "Name":"electrochemicalarrays", "Count":17 }, { "Name":"tg_propulsionelement", "Count":27 } ], "Encoded":[ { "Name":"dataminedwake", "Count":1 } ] }
{ "timestamp":"2026-08-23T20:15:06Z", "event":"MaterialCollected", "Category":"Manufactured", "Name":"tg_propulsionelement", "Count":3 }
{ "timestamp":"2026-08-23T20:24:23Z", "event":"MaterialCollected", "Category":"Manufactured", "Name":"tg_propulsionelement", "Count":3 }
{ "timestamp":"2026-08-23T21:23:47Z", "event":"MaterialTrade", "TraderType":"encoded", "Paid":{ "Material":"encryptionarchives", "Quantity":90 }, "Received":{ "Material":"hyperspacetrajectories", "Quantity":15 } }
"#;

    #[test]
    fn replays_snapshot_plus_deltas_correctly() {
        let inv = replay_lines(SAMPLE.lines());
        assert_eq!(inv.get("tg_propulsionelement"), Some(&33)); // 27 + 3 + 3
        assert_eq!(inv.get("hyperspacetrajectories"), Some(&15)); // traded in, none paid before
        assert_eq!(inv.get("tellurium"), Some(&48)); // untouched by deltas
        assert_eq!(inv.get("dataminedwake"), Some(&1));
    }

    #[test]
    fn a_later_full_snapshot_resets_rather_than_accumulates() {
        let lines = "\
{ \"event\":\"Materials\", \"Raw\":[{\"Name\":\"iron\",\"Count\":10}], \"Manufactured\":[], \"Encoded\":[] }
{ \"event\":\"MaterialCollected\", \"Name\":\"iron\", \"Count\":5 }
{ \"event\":\"Materials\", \"Raw\":[{\"Name\":\"iron\",\"Count\":2}], \"Manufactured\":[], \"Encoded\":[] }
";
        let inv = replay_lines(lines.lines());
        // must be exactly 2 (from the second snapshot), not 15 (10+5 stale-accumulated)
        assert_eq!(inv.get("iron"), Some(&2));
    }

    #[test]
    fn trade_can_zero_out_paid_material_without_going_negative() {
        let lines = "\
{ \"event\":\"Materials\", \"Raw\":[], \"Manufactured\":[], \"Encoded\":[{\"Name\":\"foo\",\"Count\":3}] }
{ \"event\":\"MaterialTrade\", \"Paid\":{\"Material\":\"foo\",\"Quantity\":10}, \"Received\":{\"Material\":\"bar\",\"Quantity\":1} }
";
        let inv = replay_lines(lines.lines());
        assert_eq!(inv.get("foo"), None); // clamped to 0 then dropped, never negative
        assert_eq!(inv.get("bar"), Some(&1));
    }
}
