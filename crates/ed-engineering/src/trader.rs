//! Material traders: turn a materials shortfall into a shopping list of
//! trades from what the commander already carries.
//!
//! Exchange rules (same trader type only -- raw, manufactured, encoded never
//! mix): within a group, one grade up costs 6:1 per step (6, 36, 216, 1296)
//! and one grade down pays 1:3 per step (3, 9, 27, 81); crossing to another
//! group multiplies the cost by a further 6. Those factors compound, so a
//! cross-group trade one grade *down* is 6:3 = 2:1 and same grade is 6:1.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TraderKind {
    Raw,
    Manufactured,
    Encoded,
}

impl TraderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "raw" => Some(Self::Raw),
            "manufactured" => Some(Self::Manufactured),
            "encoded" => Some(Self::Encoded),
            _ => None,
        }
    }
}

/// What the solver needs to know about a material.
#[derive(Debug, Clone)]
pub struct MaterialMeta {
    pub name: String,
    pub kind: TraderKind,
    /// Trader "category": the column at the trader (e.g. `Conductive`, or `4`
    /// for raw element category 4).
    pub group: String,
    /// 1..=5.
    pub grade: u8,
}

/// `give` units of `from` buy `get` units of `to`, at a `kind` trader.
#[derive(Debug, Clone, Serialize)]
pub struct Trade {
    pub kind: TraderKind,
    pub give_material: String,
    pub give: i64,
    pub get_material: String,
    pub get: i64,
    /// The trader's per-batch ratio, e.g. "6:1" or "2:3".
    pub rate: String,
    pub same_group: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShoppingList {
    pub trades: Vec<Trade>,
    /// Shortfalls that no surplus can cover, with the remaining count.
    pub still_short: Vec<(String, i64)>,
    pub traders_needed: Vec<TraderKind>,
}

/// Batch ratio give:get for one trade, reduced.
pub fn rate(from: &MaterialMeta, to: &MaterialMeta) -> Option<(i64, i64)> {
    if from.kind != to.kind || from.name.eq_ignore_ascii_case(&to.name) {
        return None;
    }
    let up = to.grade.saturating_sub(from.grade) as u32;
    let down = from.grade.saturating_sub(to.grade) as u32;
    let cross = from.group != to.group;
    let mut give = 6i64.pow(up) * if cross { 6 } else { 1 };
    let mut get = 3i64.pow(down);
    let g = gcd(give, get);
    give /= g;
    get /= g;
    Some((give, get))
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Cover `short` (material -> units missing) from `surplus` (material ->
/// units the commander can spare, i.e. inventory minus what the plan itself
/// consumes). Greedy per shortfall: the candidate costing the fewest units
/// that can cover it whole; otherwise the largest partial, then repeat.
pub fn shopping_list(
    short: &[(String, i64)],
    surplus: &HashMap<String, i64>,
    meta: &[MaterialMeta],
) -> ShoppingList {
    let by_name: HashMap<String, &MaterialMeta> =
        meta.iter().map(|m| (m.name.to_lowercase(), m)).collect();
    let mut spare: HashMap<String, i64> =
        surplus.iter().filter(|(_, v)| **v > 0).map(|(k, v)| (k.to_lowercase(), *v)).collect();
    let mut trades = Vec::new();
    let mut still_short = Vec::new();
    let mut traders_needed = Vec::new();

    for (name, qty) in short {
        let mut remaining = *qty;
        let Some(target) = by_name.get(&name.to_lowercase()) else {
            still_short.push((name.clone(), remaining));
            continue;
        };
        // Each round: best candidate for what is still missing.
        while remaining > 0 {
            let mut best: Option<(i64, i64, i64, &MaterialMeta, (i64, i64))> = None; // (get, give, spare_left, from, rate)
            for (key, have) in &spare {
                let Some(from) = by_name.get(key) else { continue };
                let Some((give_per, get_per)) = rate(from, target) else { continue };
                let batches_wanted = (remaining + get_per - 1) / get_per;
                let batches = batches_wanted.min(have / give_per);
                if batches == 0 {
                    continue;
                }
                let give = batches * give_per;
                let get = batches * get_per;
                let cand = (get, give, have - give, *from, (give_per, get_per));
                best = match best {
                    None => Some(cand),
                    Some(b) => {
                        // Cover more first; then cost less; then drain less.
                        let better = cand.0 > b.0
                            || (cand.0 == b.0 && (cand.1 < b.1 || (cand.1 == b.1 && cand.2 > b.2)));
                        Some(if better { cand } else { b })
                    }
                };
            }
            let Some((get, give, _, from, (gp, gq))) = best else { break };
            *spare.get_mut(&from.name.to_lowercase()).unwrap() -= give;
            trades.push(Trade {
                kind: target.kind,
                give_material: from.name.clone(),
                give,
                get_material: target.name.clone(),
                get,
                rate: format!("{gp}:{gq}"),
                same_group: from.group == target.group,
            });
            if !traders_needed.contains(&target.kind) {
                traders_needed.push(target.kind);
            }
            remaining -= get;
        }
        if remaining > 0 {
            still_short.push((name.clone(), remaining));
        }
    }
    ShoppingList { trades, still_short, traders_needed }
}

/// Collect something farmable, then trade it into what you need. `give`
/// units of `farm_material` (collected at `site`) trade into `get` of the
/// target; a direct pickup is `give == get` with no trade.
#[derive(Debug, Clone, Serialize)]
pub struct FarmOption {
    pub farm_material: String,
    pub site: String,
    pub system: Option<String>,
    pub body: Option<String>,
    pub collect: i64,
    pub get: i64,
    pub rate: Option<String>,
    pub kind: TraderKind,
}

/// Ways to end up with `needed` of `target` by farming. `farmable` lists
/// (material, site, system, body) for every known site. Sorted by units to
/// collect; callers may re-rank by distance.
pub fn farm_options(
    target: &MaterialMeta,
    needed: i64,
    farmable: &[(String, String, Option<String>, Option<String>)],
    meta: &[MaterialMeta],
) -> Vec<FarmOption> {
    let by_name: HashMap<String, &MaterialMeta> = meta.iter().map(|m| (m.name.to_lowercase(), m)).collect();
    let mut out = Vec::new();
    for (material, site, system, body) in farmable {
        if material.eq_ignore_ascii_case(&target.name) {
            out.push(FarmOption {
                farm_material: material.clone(), site: site.clone(), system: system.clone(), body: body.clone(),
                collect: needed, get: needed, rate: None, kind: target.kind,
            });
            continue;
        }
        let Some(from) = by_name.get(&material.to_lowercase()) else { continue };
        let Some((give_per, get_per)) = rate(from, target) else { continue };
        let batches = (needed + get_per - 1) / get_per;
        out.push(FarmOption {
            farm_material: material.clone(), site: site.clone(), system: system.clone(), body: body.clone(),
            collect: batches * give_per, get: batches * get_per,
            rate: Some(format!("{give_per}:{get_per}")), kind: target.kind,
        });
    }
    out.sort_by_key(|o| o.collect);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn farming_a_high_grade_and_trading_down_beats_a_cross_group_climb() {
        let meta = vec![
            MaterialMeta { name: "Adaptive Encryptors Capture".into(), kind: TraderKind::Encoded, group: "Encryption Files".into(), grade: 5 },
            MaterialMeta { name: "Open Symmetric Keys".into(), kind: TraderKind::Encoded, group: "Encryption Files".into(), grade: 3 },
            MaterialMeta { name: "Unexpected Emission Data".into(), kind: TraderKind::Encoded, group: "Emission Data".into(), grade: 3 },
        ];
        let farmable = vec![("Adaptive Encryptors Capture".to_string(), "Jameson".to_string(), Some("HIP 12099".to_string()), None)];
        let opts = farm_options(&meta[1], 5, &farmable, &meta);
        assert_eq!(opts[0].collect, 1, "one G5 trades down to 9 G3 in the same group");
        assert_eq!(opts[0].get, 9);
        let opts = farm_options(&meta[2], 5, &farmable, &meta);
        assert_eq!(opts[0].rate.as_deref(), Some("2:3"), "cross group two down: 6:9");
        assert_eq!(opts[0].collect, 4);
    }

    fn m(name: &str, group: &str, grade: u8) -> MaterialMeta {
        MaterialMeta { name: name.into(), kind: TraderKind::Manufactured, group: group.into(), grade }
    }

    #[test]
    fn rates_compound_as_the_trader_does() {
        let hcw = m("Heat Conduction Wiring", "Heat", 1);
        let hdp = m("Heat Dispersion Plate", "Heat", 2);
        let cc = m("Conductive Ceramics", "Conductive", 3);
        let cp = m("Conductive Polymers", "Conductive", 4);
        assert_eq!(rate(&hcw, &hdp), Some((6, 1)));
        assert_eq!(rate(&hdp, &hcw), Some((1, 3)));
        assert_eq!(rate(&hdp, &cc), Some((36, 1)), "cross group, one up: 6*6");
        assert_eq!(rate(&cp, &hdp), Some((2, 3)), "cross group, two down: 6:9");
        assert_eq!(rate(&cp, &cc), Some((1, 3)), "same group, one down");
        assert_eq!(rate(&cc, &cc), None);
    }

    #[test]
    fn the_commanders_ceramics_come_from_wiring_or_plates() {
        let meta = vec![
            m("Heat Conduction Wiring", "Heat", 1),
            m("Heat Dispersion Plate", "Heat", 2),
            m("Conductive Ceramics", "Conductive", 3),
            m("Conductive Components", "Conductive", 2),
        ];
        let surplus = HashMap::from([
            ("Heat Conduction Wiring".to_string(), 235),
            ("Heat Dispersion Plate".to_string(), 127),
            ("Conductive Components".to_string(), 10),
        ]);
        let list = shopping_list(&[("Conductive Ceramics".into(), 1)], &surplus, &meta);
        assert!(list.still_short.is_empty());
        assert_eq!(list.trades.len(), 1);
        let t = &list.trades[0];
        // Components are same group one up (6:1) -- cheapest.
        assert_eq!(t.give_material, "Conductive Components");
        assert_eq!((t.give, t.get), (6, 1));
        assert_eq!(list.traders_needed, vec![TraderKind::Manufactured]);
    }

    #[test]
    fn splits_across_sources_and_reports_what_is_left() {
        let meta = vec![m("A", "X", 1), m("B", "X", 1), m("T", "X", 2)];
        let surplus = HashMap::from([("A".to_string(), 12), ("B".to_string(), 6)]);
        let list = shopping_list(&[("T".into(), 5)], &surplus, &meta);
        assert_eq!(list.trades.iter().map(|t| t.get).sum::<i64>(), 3);
        assert_eq!(list.still_short, vec![("T".to_string(), 2)]);
    }
}
