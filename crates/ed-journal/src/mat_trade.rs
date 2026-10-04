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
//! "I need to go trade down materials maximally"; then, having done it by
//! hand: "essentially I wanted to fill up on the G1-3 mats, then rebalance
//! G4/G5 so I'd have room to accept mat rewards from missions since it's
//! all G4/5"): what to give and what to take so that the near-full G4 and
//! G5 fill the gaps below them, and whatever is still crowding a cap
//! afterwards moves into a G4 or G5 that has room. By
//! default it fills from the bottom grade up, because that is where one
//! unit goes furthest (1:81 at the bottom, 1:3 one grade down) and the
//! boss asked for "maximally"; nearest-grade-first is the option that keeps
//! the most value instead (one G5 is 1296 G1-equivalents and makes 3 G4
//! worth 648 of them, or 81 G1 worth 81). Either way: same group before
//! across, down before up, never below the source's floor, and every line
//! is a direct trade -- the game's rates make a chain cost the same, so
//! there is nothing to gain by stepping through a middle grade.

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

/// Which gap a source's surplus goes to first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillOrder {
    /// The lowest grade first: most units per source unit (the default).
    BottomFirst,
    /// The grade just below first: keeps the most value in the hold.
    NearestFirst,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    pub order: FillOrder,
    /// Only materials of this grade or above are spent (4: the G4/G5 that
    /// mission rewards crowd; 1: anything near full).
    pub min_source_grade: u8,
    /// After the gaps are filled, a source still above this share of its
    /// cap trades into a G4 or G5 with room until it is at or below it, so
    /// a reward fits. None: no such pass.
    pub room_below: Option<f64>,
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
        Policy { order: FillOrder::BottomFirst, min_source_grade: 4, room_below: Some(0.85), source_min: 0.9, floor: 0.5, cross: false, up: false }
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
    /// "down", "across", "up", or "room" (the make-room pass).
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
    pub policy_order: FillOrder,
    pub policy_min_source_grade: u8,
    pub policy_room_below: Option<f64>,
    pub policy_source_min: f64,
    pub policy_floor: f64,
    /// Materials at or above the source threshold, before trading.
    pub sources: Vec<Line>,
    pub trades: Vec<Trade>,
    /// What the plan leaves below the near-full threshold: the honest
    /// remainder. Not "below cap": 299 of 300 is a whole-trade crumb, not
    /// a shortage (boss, 2026-10-04, reading the first plan's list).
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
        .filter(|i| i.grade >= policy.min_source_grade)
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
    // One trade line: `src` into `dst`, spending down to `stop` and filling
    // up to `until`, in whole units of the trader's ratio.
    let mut trade = |have: &mut HashMap<String, i64>, src: &Item, dst: &Item, stop: i64, until: i64, direction: &'static str| {
        let Some((give, recv)) = ratio(src, dst) else { return };
        let sk = src.symbol.to_lowercase();
        let dk = dst.symbol.to_lowercase();
        if direction != "room" && is_source.contains(&dk) {
            return;
        }
        let surplus = have[&sk] - stop;
        let gap = until - have[&dk];
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

    // The grades below a source, in the order the policy fills them.
    let below = |grade: u8| -> Vec<u8> {
        match policy.order {
            FillOrder::BottomFirst => (1..grade).collect(),
            FillOrder::NearestFirst => (1..grade).rev().collect(),
        }
    };
    let floor_of = |src: &Item| floors[&src.symbol.to_lowercase()];
    // Down, own group.
    for src in &sources {
        for g in below(src.grade) {
            for dst in items.iter().filter(|i| i.group == src.group && i.grade == g) {
                trade(&mut have, src, dst, floor_of(src), cap(dst.grade), "down");
            }
        }
    }
    // Across: the same grade (6:1) first, then down the other groups.
    if policy.cross {
        for src in &sources {
            for g in std::iter::once(src.grade).chain(below(src.grade)) {
                for dst in items.iter().filter(|i| i.group != src.group && i.grade == g) {
                    trade(&mut have, src, dst, floor_of(src), cap(dst.grade), "across");
                }
            }
        }
    }
    // Up, own group, nearest grade first.
    if policy.up {
        for src in &sources {
            for g in (src.grade + 1)..=5 {
                for dst in items.iter().filter(|i| i.group == src.group && i.grade == g) {
                    trade(&mut have, src, dst, floor_of(src), cap(dst.grade), "up");
                }
            }
        }
    }
    // Room: a source still crowding its cap moves into a G4 or G5 that
    // has room, so the next mission reward fits. Own group first (G5 down
    // to G4 at 1:3, G4 up to G5 at 6:1), then across at the same grade
    // (6:1), then across one grade down (2:1). Never pushes the receiver
    // over the ceiling, never below the giver's own floor.
    if let Some(share) = policy.room_below {
        let ceiling = |i: &Item| (cap(i.grade) as f64 * share).floor() as i64;
        let candidates: Vec<&Item> = items.iter().copied().filter(|i| i.grade >= 4).collect();
        for src in sources.iter().filter(|s| s.grade >= 4) {
            let stop = ceiling(src).max(floor_of(src));
            let own: Vec<&Item> = candidates.iter().copied().filter(|i| i.group == src.group && i.symbol != src.symbol).collect();
            let same: Vec<&Item> = candidates.iter().copied().filter(|i| i.group != src.group && i.grade == src.grade).collect();
            let down: Vec<&Item> = candidates.iter().copied().filter(|i| i.group != src.group && i.grade + 1 == src.grade).collect();
            for dst in own.iter().chain(same.iter()).chain(down.iter()) {
                if have[&src.symbol.to_lowercase()] <= stop {
                    break;
                }
                trade(&mut have, src, dst, stop, ceiling(dst), "room");
            }
        }
    }
    let near_full = |i: &Item| (cap(i.grade) as f64 * policy.source_min).ceil() as i64;
    let still_short = items.iter().filter(|i| count(&have, i) < near_full(i)).map(|i| line(i, count(&have, i))).collect();
    Plan { kind: kind.to_string(), policy_order: policy.order, policy_min_source_grade: policy.min_source_grade, policy_room_below: policy.room_below, policy_source_min: policy.source_min, policy_floor: policy.floor, sources: source_lines, trades, still_short }
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

    /// The boss's Chemical group on 2026-10-04 (Pharmaceutical Isolators 99,
    /// Storage Units 27, Processors 4, Distillery 3, Manipulators 0): bottom
    /// first, 49 spare G5 fill three grades to within a trade of their caps
    /// and still make 48 Manipulators. Nearest first would have put all 49
    /// into Manipulators at 1:3 and never reached the bottom (the boss:
    /// "why wouldn't I just trade a top mat for a bottom mat instead of
    /// doing the intermediary?").
    #[test]
    fn bottom_first_fills_the_most_from_the_same_surplus() {
        let c = cat();
        let have = inv(&[("pharmaceuticalisolators", 99), ("chemicalstorageunits", 27), ("chemicalprocessors", 4), ("chemicaldistillery", 3), ("chemicalmanipulators", 0)]);
        let p = plan(&c, &have, "manufactured", &Policy { room_below: None, ..Policy::default() }, None);
        let t: Vec<(&str, i64, i64)> = p.trades.iter().map(|t| (t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        assert_eq!(t, [("chemicalstorageunits", 3, 243), ("chemicalprocessors", 9, 243), ("chemicaldistillery", 21, 189), ("chemicalmanipulators", 16, 48)]);
        assert_eq!(p.trades.iter().map(|t| t.give_qty).sum::<i64>(), 49, "every spare unit spent");
        let near = plan(&c, &have, "manufactured", &Policy { order: FillOrder::NearestFirst, room_below: None, ..Policy::default() }, None);
        assert_eq!(near.trades.len(), 1);
        assert_eq!((near.trades[0].recv_symbol.as_str(), near.trades[0].give_qty, near.trades[0].recv_qty), ("chemicalmanipulators", 49, 147));
    }

    /// Full G5 thermic alloys, empty below: nearest first spends down to the
    /// floor on the grade just below and stops at its cap.
    #[test]
    fn trading_down_fills_the_nearest_grade_first_and_keeps_the_floor() {
        let c = cat();
        let p = plan(&c, &inv(&[("militarygradealloys", 100), ("thermicalloys", 0), ("precipitatedalloys", 0), ("heatresistantceramics", 0), ("temperedalloys", 0)]), "manufactured", &Policy { order: FillOrder::NearestFirst, room_below: None, ..Policy::default() }, None);
        assert_eq!(p.sources.len(), 1);
        let t: Vec<(&str, i64, i64)> = p.trades.iter().map(|t| (t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // 50 to spend: 50 -> 150 thermic alloys (cap). Nothing left for the rest.
        assert_eq!(t, [("thermicalloys", 50, 150)]);
        assert_eq!(p.trades[0].give_after, 50, "the floor is half the cap");
        assert!(p.still_short.iter().any(|l| l.symbol == "precipitatedalloys"), "the honest remainder");
        assert!(!p.still_short.iter().any(|l| l.symbol == "thermicalloys"), "filled to cap: not short");
        let crumbs = plan(&c, &inv(&[("militarygradealloys", 100), ("thermicalloys", 149), ("precipitatedalloys", 199), ("heatresistantceramics", 249), ("temperedalloys", 299)]), "manufactured", &Policy { room_below: None, ..Policy::default() }, None);
        assert!(!crumbs.still_short.iter().any(|l| ["thermicalloys", "precipitatedalloys", "heatresistantceramics", "temperedalloys"].contains(&l.symbol.as_str())), "one short of a cap is not a shortage: {:?}", crumbs.still_short.iter().map(|l| (&l.name, l.count)).collect::<Vec<_>>());
    }

    #[test]
    fn a_gap_is_filled_in_whole_trades_from_every_full_material() {
        let c = cat();
        let p = plan(&c, &inv(&[("militarygradealloys", 100), ("thermicalloys", 150), ("precipitatedalloys", 191), ("heatresistantceramics", 250), ("temperedalloys", 100)]), "manufactured", &Policy { order: FillOrder::NearestFirst, room_below: None, ..Policy::default() }, None);
        let t: Vec<(&str, i64, i64)> = p.trades.iter().map(|t| (t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // Only G4 and G5 are spent by default. Nearest first: the G5 fills
        // precipitated's gap of 9 (one trade at 1:9), then tempered's 200
        // takes two trades at 1:81 (a third would overflow); the full G4
        // adds one at 1:27. Every spendable material contributes.
        assert_eq!(t, [("precipitatedalloys", 1, 9), ("temperedalloys", 2, 162), ("temperedalloys", 1, 27)]);
    }

    #[test]
    fn across_and_up_come_after_down_and_only_when_allowed() {
        let c = cat();
        // Raw: yttrium (G4, group 1) full; group 1 below it full; group 2 wants.
        let full_group1 = [("yttrium", 150), ("niobium", 200), ("vanadium", 250), ("carbon", 300)];
        let mut have: Vec<(&str, i64)> = full_group1.to_vec();
        have.extend([("technetium", 120), ("molybdenum", 0)]);
        let off = plan(&c, &inv(&have), "raw", &Policy { room_below: None, ..Policy::default() }, None);
        assert!(off.trades.is_empty(), "nothing to do in its own group");
        let on = plan(&c, &inv(&have), "raw", &Policy { cross: true, up: true, room_below: None, ..Policy::default() }, None);
        let first = &on.trades[0];
        // Yttrium first (highest grade), same grade across first: 6:1, twelve
        // whole trades of the 75 it may spend; the other full materials follow.
        assert_eq!((first.direction, first.recv_symbol.as_str(), first.give_qty, first.recv_qty), ("across", "technetium", 72, 12));
        assert!(on.trades.iter().all(|t| t.direction == "across" && t.give_symbol == "yttrium"), "{:?}", on.trades.iter().map(|t| (t.direction, &t.give_symbol)).collect::<Vec<_>>());
        let yttrium: i64 = on.trades.iter().map(|t| t.give_qty).sum();
        assert!(yttrium <= 75, "never below the floor of 75: gave {yttrium}");
        assert!(on.trades.iter().all(|t| t.recv_symbol != "yttrium" && t.recv_symbol != "niobium"), "a source never receives");
    }

    #[test]
    fn up_trades_fill_the_grade_above_from_a_full_low_grade() {
        let c = cat();
        let p = plan(&c, &inv(&[("iron", 300), ("zinc", 200), ("tin", 100), ("selenium", 50)]), "raw", &Policy { min_source_grade: 1, up: true, room_below: None, ..Policy::default() }, None);
        let t: Vec<(&str, &str, i64, i64)> = p.trades.iter().map(|t| (t.direction, t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // 150 iron to spend, 6:1 up: 25 zinc fills half the gap and the iron is at its floor.
        assert_eq!(t, [("up", "zinc", 150, 25)]);
    }

    /// The boss's actual aim (2026-10-04): G1-3 full, then room in G4/G5
    /// for mission rewards. With everything below full, a G5 at its cap
    /// moves into G4/G5 that have room -- own group first, then across at
    /// the same grade, then one down -- until it is under the ceiling, and
    /// never pushes a receiver over it.
    #[test]
    fn the_room_pass_makes_space_in_the_crowded_grades() {
        let c = cat();
        let mut have = inv(&[("militarygradealloys", 100), ("thermicalloys", 150), ("precipitatedalloys", 200), ("heatresistantceramics", 250), ("temperedalloys", 300)]);
        have.insert("protoheatradiators".into(), 40); // another group's G5, room for 45 under the 85 % ceiling
        have.insert("heatvanes".into(), 120); // its G4: 127 is the ceiling, room for 7
        let p = plan(&c, &have, "manufactured", &Policy::default(), None);
        let t: Vec<(&str, &str, i64, i64)> = p.trades.iter().map(|t| (t.direction, t.recv_symbol.as_str(), t.give_qty, t.recv_qty)).collect();
        // MGA: 100 -> ceiling 85, 15 to move. Own group is full (thermic is a
        // source, over its own ceiling). Across at the same grade, 6:1: two
        // whole trades (12) into the first G5 with room; 3 left, one down at
        // 2:1: one trade into a G4 with room. MGA ends at 86, within one
        // whole trade of the ceiling. Thermic alloys (150, ceiling 127) then
        // moves across the same way; nothing ever goes into a G1-G3.
        assert!(t.iter().all(|x| x.0 == "room"), "{t:?}");
        assert_eq!((t[0].2, t[0].3, p.trades[0].recv_grade, p.trades[0].ratio.as_str()), (12, 2, 5, "6:1"), "{t:?}");
        assert_eq!((t[1].2, t[1].3, p.trades[1].recv_grade, p.trades[1].ratio.as_str()), (2, 1, 4, "2:1"), "{t:?}");
        assert!(p.trades.iter().all(|x| x.recv_grade >= 4), "room moves only into G4/G5: {t:?}");
        assert!(p.trades.iter().all(|x| x.recv_after <= (cap(x.recv_grade) as f64 * 0.85).floor() as i64), "never over the receiver's ceiling: {t:?}");
        let mga = p.trades.iter().filter(|x| x.give_symbol == "militarygradealloys").last().unwrap();
        assert_eq!(mga.give_after, 86, "within one whole trade of the ceiling");
        assert!(p.trades.iter().any(|x| x.give_symbol == "thermicalloys"), "the full G4 makes room too: {t:?}");
        let quiet = plan(&c, &have, "manufactured", &Policy { room_below: None, ..Policy::default() }, None);
        assert!(quiet.trades.is_empty(), "without the room pass there is nothing to fill");
    }

    /// The callout's question: one material just crossed the threshold —
    /// is there a trade for it?
    #[test]
    fn the_single_source_question_is_silent_when_everything_below_is_full() {
        let c = cat();
        let full = inv(&[("militarygradealloys", 95), ("thermicalloys", 150), ("precipitatedalloys", 200), ("heatresistantceramics", 250), ("temperedalloys", 300)]);
        let p = plan(&c, &full, "manufactured", &Policy { room_below: None, ..Policy::default() }, Some("militarygradealloys"));
        assert!(p.trades.is_empty());
        let mut gap = full.clone();
        gap.insert("precipitatedalloys".into(), 100);
        let p = plan(&c, &gap, "manufactured", &Policy { room_below: None, ..Policy::default() }, Some("militarygradealloys"));
        assert_eq!(p.trades.len(), 1);
        assert_eq!((p.trades[0].recv_symbol.as_str(), p.trades[0].give_qty, p.trades[0].recv_qty), ("precipitatedalloys", 11, 99));
    }
}
