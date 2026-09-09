//! Walk the goal field's own greedy descent and measure whether each
//! cell step is realizable by actual star hops (item 18d).
//!
//!     field_probe <index_dir> <from> <to> [--min-stars N]
//!
//! Builds the chain-floored field toward <to>, then from <from>'s cell
//! repeatedly steps to the lowest-field neighbour, printing per step the
//! field value, star count, and the MINIMUM star-pair distance from the
//! previous cell -- the hop a ship would actually have to fly. Steps
//! where that minimum exceeds boosted reach are the field's broken
//! promises: cell-level connectivity no star chain delivers.

use ed_galaxy::Galaxy;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or(".data/galaxy"));
    let g = Galaxy::open(dir)?;
    let sub = Galaxy::open(&ed_galaxy::long_range::neutron_dir(dir))?;
    let cg = sub.cell_graph().ok_or_else(|| anyhow::anyhow!("no graph250.bin"))?;
    let from = g.find(&args[1]).ok_or_else(|| anyhow::anyhow!("unknown from"))?;
    let to = g.find(&args[2]).ok_or_else(|| anyhow::anyhow!("unknown to"))?;
    let min_stars: u32 = args
        .iter()
        .position(|a| a == "--min-stars")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let (from_pos, to_pos) = (g.pos_of(from), g.pos_of(to));
    let from_cell = sub
        .cell_index_of_pos(from_pos)
        .or_else(|| ed_galaxy::cgraph::nearest_cell(&sub, from_pos, 2_000.0))
        .ok_or_else(|| anyhow::anyhow!("no start cell"))?;
    let to_cell = sub
        .cell_index_of_pos(to_pos)
        .or_else(|| ed_galaxy::cgraph::nearest_cell(&sub, to_pos, 2_000.0))
        .ok_or_else(|| anyhow::anyhow!("no goal cell"))?;
    let keep = |c: usize| c == from_cell || c == to_cell || sub.cell_records(c).is_some_and(|(_, n)| n >= min_stars);
    let field = cg.goal_field_where(to_cell, keep);
    eprintln!(
        "field toward {} (cell {to_cell}), floor {min_stars}; start cell {from_cell} field {:?}",
        args[2],
        field.jumps_to_goal(from_cell)
    );

    let stars_of = |cell: usize| -> Vec<[f32; 3]> {
        sub.cell_records(cell)
            .map(|(s0, n)| (s0..s0 + n).map(|i| sub.pos_of(i)).collect())
            .unwrap_or_default()
    };
    let min_pair = |a: &[[f32; 3]], b: &[[f32; 3]]| -> f32 {
        let mut best = f32::MAX;
        for pa in a {
            for pb in b {
                best = best.min(ed_galaxy::format::dist(*pa, *pb));
            }
        }
        best
    };

    println!(
        "{:>4} {:>8} {:>7} {:>6} {:>10} {:>8}",
        "step", "field", "stars", "kept", "min-hop", "verdict"
    );
    let mut cur = from_cell;
    let mut prev_stars = stars_of(cur);
    let mut broken = 0u32;
    for step in 0..400 {
        if cur == to_cell {
            println!("reached the goal cell at step {step}, {broken} broken steps");
            return Ok(());
        }
        let curf = field.jumps_to_goal(cur).unwrap_or(f32::MAX);
        // Greedy descent: the neighbour with the lowest field value.
        let mut best: Option<(f32, usize)> = None;
        for &(nc, _) in cg.neighbours(cur) {
            if let Some(nf) = field.jumps_to_goal(nc as usize) {
                if best.is_none_or(|(bf, _)| nf < bf) {
                    best = Some((nf, nc as usize));
                }
            }
        }
        let Some((nf, next)) = best else {
            println!("step {step}: no descending neighbour from cell {cur} (field {curf:.1}) -- dead end");
            return Ok(());
        };
        if nf >= curf {
            println!("step {step}: local minimum at cell {cur} (field {curf:.1}, best neighbour {nf:.1})");
            return Ok(());
        }
        let next_stars = stars_of(next);
        let hop = min_pair(&prev_stars, &next_stars);
        let kept = keep(next);
        let verdict = if hop <= 467.0 { "ok" } else { broken += 1; "BROKEN" };
        if hop > 467.0 || step < 12 || step % 20 == 0 {
            println!(
                "{:>4} {:>8.2} {:>7} {:>6} {:>10.1} {:>8}",
                step,
                nf,
                next_stars.len(),
                kept,
                hop,
                verdict
            );
        }
        prev_stars = next_stars;
        cur = next;
    }
    println!("walk did not reach the goal cell in 400 steps; {broken} broken steps");
    Ok(())
}
