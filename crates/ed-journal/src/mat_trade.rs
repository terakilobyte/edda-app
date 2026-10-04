//! Material trader arithmetic and the trade-down planner.
//!
//! The ratios are the game's, measured: every one of the commander's 114
//! `MaterialTrade` events replays exactly against this model
//! (`docs/benches/2026-10-04-material-trade-ratios.csv`, knob
//! `docs/benches/knobs/material_trade_ratios.py`). Same trader group, `d`
//! grades down: 1 gives 3^d. Same group, `u` grades up: 6^u gives 1. Across
//! groups of the same trader type: same grade 6 gives 1; `d` down: 2 gives
//! 3^(d-1); `u` up: 6^(u+1) gives 1. Guardian and Thargoid materials (group
//! `None`) do not trade.
//!
//! The planner answers the question asked at the trader (boss, 2026-10-04:
//! "I need to go trade down materials maximally"): what to give and what to
//! take so that materials nearing their cap fill the gaps below them. It
//! spends the nearest lower grade first (that keeps the most value: one G5
//! is 1296 G1-equivalents and makes 3 G4 worth 648 of them, or 81 G1 worth
//! 81), same group before across, down before up, and never takes a
//! source below its floor.

use crate::catalog::{Catalog, Item, Kind};
use std::collections::HashMap;

/// The game's storage cap by grade.
pub fn cap(grade: u8) -> i64 {
    match grade {
        1 => 300,
        2 => 250,
        3 => 200,
        4 => 150,
        _ => 100,
    }
}

/// Whether a trader will take or give this at all.
pub fn tradeable(i: &Item) -> bool {
    i.kind == Kind::Material && !i.group.is_empty() && i.group != "None" && (1..=5).contains(&i.grade)
}

/// (give, receive) for the smallest trade of `src` into `dst`; None when the
/// trader will not (different trader type, same material, untradeable).
pub fn ratio(src: &Item, dst: &Item) -> Option<(i64, i64)> {
    if !tradeable(src) || !tradeable(dst) || !src.category.eq_ignore_ascii_case(&dst.category) {
        return None;
    }
    let d = src.grade as i32 - dst.grade as i32;
    if src.group == dst.group {
        return match d {
            0 => None,
            d if d > 0 => Some((1, 3i64.pow(d as u32))),
            d => Some((6i64.pow((-d) as u32), 1)),
        };
    }
    Some(match d {
        0 => (6, 1),
        d if d > 0 => (2, 3i64.pow((d - 1) as u32)),
        d => (6i64.pow((-d + 1) as u32), 1),
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    /// A material at this fraction of its cap or above is a source.
    pub source_min: f64,
    /// Sources are never spent below this fraction of their cap.
    pub floor: f64,
    /// Trade across groups (six times dearer) once the own group is filled.
    pub cross: bool,
    /// Trade up within the group (6:1 per grade) once down is done.
    pub up: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { source_min: 0.9, floor: 0.5, cross: true, up: true }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Trade {
    pub give_symbol: String,
    pub give_name: String,
    pub give_grade: u8,
    pub give_qty: i64,
    pub give_before: i64,
    pub give_after: i64,
    pub recv_symbol: String,
    pub recv_name: String,
    pub recv_grade: u8,
    pub recv_qty: i64,
    pub recv_before: i64,
    pub recv_after: i64,
    /// "down", "across" or "up".
    pub direction: &'static str,
    /// The trader's unit, "1:27".
    pub ratio: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Line {
    pub symbol: String,
    pub name: String,
    pub group: String,
    pub grade: u8,
    pub count: i64,
    pub cap: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Plan {
    pub kind: String,
    pub policy_source_min: f64,
    pub policy_floor: f64,
    /// Materials at or above the source threshold, before trading.
    pub sources: Vec<Line>,
    pub trades: Vec<Trade>,
    /// Gaps left after the plan (below cap), for the honest remainder.
    pub still_short: Vec<Line>,
}

fn line(i: &Item, count: i64) -> Line {
    Line { symbol: i.symbol.to_lowercase(), name: i.name.clone(), group: i.group.clone(), grade: i.grade, count, cap: cap(i.grade) }
}

/// The plan for one trader type. `inventory` is lowercase symbol -> count;
/// `only` restricts the sources to one material (the callout's question).
pub fn plan(catalog: &Catalog, inventory: &HashMap<String, i64>, kind: &str, policy: &Policy, only: Option<&str>) -> Plan {
    let mut items: Vec<&Item> = catalog.materials().filter(|i| tradeable(i) && i.category.eq_ignore_ascii_case(kind)).collect();
    items.sort_by(|a, b| a.group.cmp(&b.group).then(a.grade.cmp(&b.grade)).then(a.symbol.cmp(&b.symbol)));
    let mut have: HashMap<String, i64> = items.iter().map(|i| (i.symbol.to_lowercase(), inventory.get(&i.symbol.to_lowercase()).copied().unwrap_or(0))).collect();
    let count = |have: &HashMap<String, i64>, i: &Item| have[&i.symbol.to_lowercase()];

    let mut sources: Vec<&Item> = items
        .iter()
        .copied()
        .filter(|i| only.is_none_or(|o| i.symbol.eq_ignore_ascii_case(o)))
        .filter(|i| (count(&have, i) as f64) >= (cap(i.grade) as f64 * policy.source_min).ceil())
        .collect();
    // Highest grade first: its surplus is the dearest to leave idle.
    sources.sort_by(|a, b| b.grade.cmp(&a.grade).then(a.group.cmp(&b.group)));
    let floors: HashMap<String, i64> = sources.iter().map(|i| (i.symbol.to_lowercase(), (cap(i.grade) as f64 * policy.floor).floor() as i64)).collect();
    // A source never receives: topping up something already near full
    // from another source is churn (iron up to zinc so zinc can go up to
    // tin so tin can refill the selenium that filled zinc).
    let is_source: std::collections::HashSet<String> = sources.iter().map(|i| i.symbol.to_lowercase()).collect();
    let source_lines: Vec<Line> = sources.iter().map(|i| line(i, count(&have, i))).collect();

    let mut trades: Vec<Trade> = Vec::new();
    let mut trade = |have: &mut HashMap<String, i64>, src: &Item, dst: &Item, direction: &'static str| {
        let Some((give, recv)) = ratio(src, dst) else { return };
        let sk = src.symbol.to_lowercase();
        let dk = dst.symbol.to_lowercase();
        if is_source.contains(&dk) {
            return;
        }
        let surplus = have[&sk] - floors[&sk];
        let gap = cap(dst.grade) - have[&dk];
        let n = (surplus / give).min(gap / recv);
        if n <= 0 {
            return;
        }
        let (gb, rb) = (have[&sk], have[&dk]);
        *have.get_mut(&sk).unwrap() -= n * give;
        *have.get_mut(&dk).unwrap() += n * recv;
        trades.push(Trade {
            give_symbol: sk.clone(),
            give_name: src.name.clone(),
            give_grade: src.grade,
            give_qty: n * give,
            give_before: gb,
            give_after: have[&sk],
            recv_symbol: dk.clone(),
            recv_name: dst.name.clone(),
            recv_grade: dst.grade,
            recv_qty: n * recv,
            recv_before: rb,
            recv_after: have[&dk],
            direction,
            ratio: format!("{give}:{recv}"),
        });
    };

    // Down, own group, nearest grade first.
    for src in &sources {
        for d in 1..src.grade {
            let g = src.grade - d;
            for dst in items.iter().filter(|i| i.group == src.group && i.grade == g) {
                trade(&mut have, src, dst, "down");
            }
        }
    }
    // Across: same grade (6:1), then down the other groups.
    if policy.cross {
        for src in &sources {
            for g in (1..=src.grade).rev() {
                for dst in items.iter().filter(|i| i.group != src.group && i.grade == g) {
                    trade(&mut have, src, dst, "across");
                }
            }
        }
    }
    // Up, own group, nearest grade first.
    if policy.up {
        for src in &sources {
            for g in (src.grade + 1)..=5 {
                for dst in items.iter().filter(|i| i.group == src.group && i.grade == g) {
                    trade(&mut have, src, dst, "up");
                }
            }
        }
    }
    let still_short = items.iter().filter(|i| count(&have, i) < cap(i.grade)).map(|i| line(i, count(&have, i))).collect();
    Plan { kind: kind.to_string(), policy_source_min: policy.source_min, policy_floor: policy.floor, sources: source_lines, trades, still_short }
}

/// The trader types, as the journal spells them.
pub const KINDS: &[&str] = &["raw", "manufactured", "encoded"];

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalog {
        Catalog::load()
    }
    fn item<'a>(c: &'a Catalog, s: &str) -> &'a Item {
        c.by_symbol(s).unwrap_or_else(|| panic!("{s} not in the catalog"))
    }

    /// Nine trades from the commander's journal, one per cell of the model
    /// (`docs/benches/2026-10-04-material-trade-ratios.csv`, all 114 agree).
    #[test]
    fn ratios_are_the_journals() {
        let c = cat();
        let r = |a: &str, b: &str| ratio(item(&c, a), item(&c, b));
        assert_eq!(r("militarygradealloys", "temperedalloys"), Some((1, 81)), "G5 to G1, same group: 3 gave 243");
        assert_eq!(r("militarygradealloys", "thermicalloys"), Some((1, 3)), "49 gave 147");
        assert_eq!(r("heatvanes", "heatdispersionplate"), Some((1, 9)), "15 gave 135");
        assert_eq!(r("yttrium", "technetium"), Some((6, 1)), "across, same grade: 6 gave 1");
        assert_eq!(r("yttrium", "molybdenum"), Some((2, 1)), "across, one down: 4 gave 2");
        assert_eq!(r("antimony", "arsenic"), Some((2, 3)), "across, two down: 6 gave 9");
        assert_eq!(r("yttrium", "phosphorus"), Some((2, 9)), "across, three down: 2 gave 9");
        assert_eq!(r("adaptiveencryptors", "dataminedwake"), Some((6, 1)), "encoded across: 24 gave 4");
        assert_eq!(r("temperedalloys", "heatresistantceramics"), Some((6, 1)), "up one, same group");
        assert_eq!(r("temperedalloys", "militarygradealloys"), Some((1296, 1)));
        assert_eq!(r("temperedalloys", "heatconductionwiring"), Some((6, 1)), "G1 to G1 across groups");
    }

    #[test]
    fn what_will_not_trade() {
        let c = cat();
        let r = |a: &str, b: &str| ratio(item(&c, a), item(&c, b));
        assert_eq!(r("iron", "temperedalloys"), None, "raw never becomes manufactured");
        assert_eq!(r("iron", "iron"), None);
        assert_eq!(r("guardian_powerconduit", "temperedalloys"), None, "Guardian materials do not trade");
        assert_eq!(r("temperedalloys", "guardian_powerconduit"), None);
    }

    fn inv(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    /// Full G5 thermic alloys, empty below: the plan spends down to the
    /// floor, nearest grade first, and stops at each cap.
    #[test]
    fn trading_down_fills_the_nearest_grade_first_and_keeps_the_floor() {
        let c = cat();
        let p = plan(&c, &inv(&[("militarygradealloys", 100), ("thermicalloys", 0), ("precipitatedalloys", 0), ("heatresistantceramics", 0), ("temperedalloys", 0)]), "manufactured", &Policy { cross: false, up: false, ..Policy::default() }, None);
        assert_eq!(p.sources.len(), 1);
        let t: Vec<(&str, i64, i64)> = p.trades.iter().map(|t| (t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // 50 to spend: 50 -> 150 thermic alloys (cap). Nothing left for the rest.
        assert_eq!(t, [("thermicalloys", 50, 150)]);
        assert_eq!(p.trades[0].give_after, 50, "the floor is half the cap");
        assert!(p.still_short.iter().any(|l| l.symbol == "precipitatedalloys"), "the honest remainder");
    }

    #[test]
    fn a_gap_is_filled_in_whole_trades_from_every_full_material() {
        let c = cat();
        let p = plan(&c, &inv(&[("militarygradealloys", 100), ("thermicalloys", 150), ("precipitatedalloys", 191), ("heatresistantceramics", 250), ("temperedalloys", 100)]), "manufactured", &Policy { cross: false, up: false, ..Policy::default() }, None);
        let t: Vec<(&str, i64, i64)> = p.trades.iter().map(|t| (t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // Precipitated is itself near full (191 of 200), so it is a source, not a target.
        // Tempered's gap of 200: two G5 trades at 1:81 (162; a third would overflow),
        // then one G4 at 1:27, then one G3 at 1:9 -- every full material contributes.
        assert_eq!(t, [("temperedalloys", 2, 162), ("temperedalloys", 1, 27), ("temperedalloys", 1, 9)]);
    }

    #[test]
    fn across_and_up_come_after_down_and_only_when_allowed() {
        let c = cat();
        // Raw: yttrium (G4, group 1) full; group 1 below it full; group 2 wants.
        let full_group1 = [("yttrium", 150), ("niobium", 200), ("vanadium", 250), ("carbon", 300)];
        let mut have: Vec<(&str, i64)> = full_group1.to_vec();
        have.extend([("technetium", 120), ("molybdenum", 0)]);
        let off = plan(&c, &inv(&have), "raw", &Policy { cross: false, up: false, ..Policy::default() }, None);
        assert!(off.trades.is_empty(), "nothing to do in its own group");
        let on = plan(&c, &inv(&have), "raw", &Policy::default(), None);
        let first = &on.trades[0];
        // Yttrium first (highest grade), same grade across first: 6:1, twelve
        // whole trades of the 75 it may spend; the other full materials follow.
        assert_eq!((first.direction, first.recv_symbol.as_str(), first.give_qty, first.recv_qty), ("across", "technetium", 72, 12));
        assert!(on.trades.iter().all(|t| t.direction == "across"), "{:?}", on.trades.iter().map(|t| t.direction).collect::<Vec<_>>());
        let yttrium: i64 = on.trades.iter().filter(|t| t.give_symbol == "yttrium").map(|t| t.give_qty).sum();
        assert!(yttrium <= 75, "never below the floor of 75: gave {yttrium}");
        assert!(on.trades.iter().all(|t| t.recv_symbol != "yttrium" && t.recv_symbol != "niobium"), "a source never receives");
    }

    #[test]
    fn up_trades_fill_the_grade_above_from_a_full_low_grade() {
        let c = cat();
        let p = plan(&c, &inv(&[("iron", 300), ("zinc", 200), ("tin", 100), ("selenium", 50)]), "raw", &Policy { cross: false, ..Policy::default() }, None);
        let t: Vec<(&str, &str, i64, i64)> = p.trades.iter().map(|t| (t.direction, t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // 150 iron to spend, 6:1 up: 25 zinc fills half the gap and the iron is at its floor.
        assert_eq!(t, [("up", "zinc", 150, 25)]);
    }

    /// The callout's question: one material just crossed the threshold —
    /// is there a trade for it?
    #[test]
    fn the_single_source_question_is_silent_when_everything_below_is_full() {
        let c = cat();
        let full = inv(&[("militarygradealloys", 95), ("thermicalloys", 150), ("precipitatedalloys", 200), ("heatresistantceramics", 250), ("temperedalloys", 300)]);
        let p = plan(&c, &full, "manufactured", &Policy { cross: false, up: false, ..Policy::default() }, Some("militarygradealloys"));
        assert!(p.trades.is_empty());
        let mut gap = full.clone();
        gap.insert("precipitatedalloys".into(), 100);
        let p = plan(&c, &gap, "manufactured", &Policy { cross: false, up: false, ..Policy::default() }, Some("militarygradealloys"));
        assert_eq!(p.trades.len(), 1);
        assert_eq!((p.trades[0].recv_symbol.as_str(), p.trades[0].give_qty, p.trades[0].recv_qty), ("precipitatedalloys", 11, 99));
    }
}
