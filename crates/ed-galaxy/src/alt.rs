//! `alt250.bin` -- the ALT (A*, Landmarks, Triangle inequality) lower
//! bound over the boost sub-index's occupied cells, per ROUTING-NEXT
//! item 5.2. The straight-line heuristic misleads the coarse search
//! wherever the highway detours -- voids, the rim, Beagle Point -- and
//! the search pays for it in widened rescans (~65 % of coarse time on
//! the rim, measured). This bound knows about the detours.
//!
//! **Graph:** the occupied 250 ly cells; two cells are adjacent when
//! their centres are within [`R_REF`] = 600 ly (at least any ship's
//! boosted reach, so the cell-graph distance lower-bounds the highway
//! distance for every loadout -- one table serves all ships); edge cost
//! = centre distance in ly. **Landmarks:** [`LANDMARKS`] cells by
//! farthest-point sampling seeded from the cell nearest Sol, one
//! Dijkstra each, run at highway-rebuild time. **Query:**
//! `max_L |d(L, goal) - d(L, v)|` minus [`SLACK_LY`] (twice the cell
//! diagonal, so cell granularity cannot make the bound inadmissible),
//! floored at zero; the caller keeps the straight-line floor.
//!
//! Optional derived data like `agg250.bin`, stored INSIDE the
//! sub-index directory so the staging swap and deletion cover it;
//! absence = the straight-line heuristic alone, exactly as before.

use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::format::{morton_cell_of, Galaxy};

pub const ALT_FILE: &str = "alt250.bin";
const MAGIC: &[u8; 4] = b"ALT1";
pub const LANDMARKS: usize = 16;
/// Largest plausible boosted reach (Caspian x6 at ~100 ly).
pub const R_REF: f32 = 600.0;
/// Distances are stored in units of 8 ly, saturating below the sentinel.
const UNIT_LY: f32 = 8.0;
const UNREACHABLE: u16 = u16::MAX;
/// Twice the 250 ly cell diagonal: the bound's admissibility slack.
pub const SLACK_LY: f32 = 866.0;

pub struct AltOracle {
    /// `cells * LANDMARKS` distances in [`UNIT_LY`] units, cell-major,
    /// parallel to the sub-index's morton-ordered cell array.
    table: Vec<u16>,
    landmark_cells: Vec<u32>,
    cells: usize,
}

impl AltOracle {
    pub fn leaf_count(&self) -> usize {
        self.cells
    }

    pub fn landmarks(&self) -> &[u32] {
        &self.landmark_cells
    }

    /// Lower bound in ly on the highway travel distance between the two
    /// occupied cells, from the triangle inequality over every landmark
    /// that reaches both. Zero when nothing binds (disconnected
    /// component, or the cells are simply close).
    pub fn lower_bound_ly(&self, from_cell: usize, goal_cell: usize) -> f32 {
        debug_assert!(from_cell < self.cells && goal_cell < self.cells);
        let mut best = 0u16;
        for l in 0..self.landmark_cells.len() {
            let dv = self.table[from_cell * self.landmark_cells.len() + l];
            let dg = self.table[goal_cell * self.landmark_cells.len() + l];
            if dv == UNREACHABLE || dg == UNREACHABLE {
                continue;
            }
            best = best.max(dv.abs_diff(dg));
        }
        (best as f32 * UNIT_LY - SLACK_LY).max(0.0)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut out = Vec::with_capacity(16 + self.table.len() * 2);
        out.extend_from_slice(MAGIC);
        out.push(1u8);
        out.push(self.landmark_cells.len() as u8);
        out.extend_from_slice(&[0u8; 2]);
        out.extend_from_slice(&(self.cells as u32).to_le_bytes());
        out.extend_from_slice(&R_REF.to_le_bytes());
        for &l in &self.landmark_cells {
            out.extend_from_slice(&l.to_le_bytes());
        }
        for &d in &self.table {
            out.extend_from_slice(&d.to_le_bytes());
        }
        let mut file = std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        file.write_all(&out)?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<AltOracle> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.len() < 16 || &bytes[0..4] != MAGIC {
            bail!("{} is not an ALT1 table", path.display());
        }
        ensure!(bytes[4] == 1, "unknown ALT version {}", bytes[4]);
        let landmarks = bytes[5] as usize;
        ensure!((1..=64).contains(&landmarks), "implausible landmark count {landmarks}");
        let cells = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut at = 16;
        let mut landmark_cells = Vec::with_capacity(landmarks);
        for _ in 0..landmarks {
            let end = at + 4;
            ensure!(bytes.len() >= end, "{} is truncated", path.display());
            landmark_cells.push(u32::from_le_bytes(bytes[at..end].try_into().unwrap()));
            at = end;
        }
        let need = cells * landmarks * 2;
        ensure!(bytes.len() == at + need, "{} carries {} table bytes for {cells} cells x {landmarks}", path.display(), bytes.len() - at);
        let table = bytes[at..]
            .as_chunks::<2>().0.iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok(AltOracle { table, landmark_cells, cells })
    }
}

fn centre(key: u64, cell_ly: f32) -> [f32; 3] {
    let (cx, cy, cz) = morton_cell_of(key);
    [(cx as f32 + 0.5) * cell_ly, (cy as f32 + 0.5) * cell_ly, (cz as f32 + 0.5) * cell_ly]
}

/// Build the table over the COARSE CELL GRAPH's metric -- highway edges
/// at boosted price plus gap edges at plain price -- so the landmark
/// distances carry true void-crossing costs. The first trial ran over a
/// highway-only ly metric and measured null on the real galaxy (the
/// straight line never lost); the desert widen (5.3-v2) changed the
/// question: voids are now crossable, euclid actively misleads across
/// them, and only a gap-aware metric prices them honestly. Distances
/// are stored as boosted-ly equivalents (jumps x the reference boosted
/// hop) so the heuristic's ly arithmetic is unchanged and remains a
/// lower bound for every real ship.
pub fn build(sub: &Galaxy, graph: &crate::cgraph::CellGraph) -> Result<AltOracle> {
    ensure!(sub.morton_cells(), "the ALT table needs a v3 (morton-ordered) index");
    let cells = sub.cell_count();
    ensure!(cells > 0, "an empty sub-index has nothing to bound");
    ensure!(graph.leaf_count() == cells, "the cell graph belongs to another sub-index");
    let cell_ly = sub.cell_ly;
    let centres: Vec<[f32; 3]> = (0..cells).map(|i| centre(sub.cell_entry(i).0, cell_ly)).collect();
    let dijkstra = |from: usize| -> Vec<f32> {
        graph
            .goal_field(from)
            .as_jumps()
            .into_iter()
            .map(|j| if j.is_finite() { j * crate::cgraph::REF_BOOSTED_REACH_LY } else { f32::INFINITY })
            .collect()
    };
    // Seed: the occupied cell nearest Sol (the origin).
    let seed = (0..cells)
        .min_by(|&a, &b| {
            let da = centres[a].iter().map(|v| v * v).sum::<f32>();
            let db = centres[b].iter().map(|v| v * v).sum::<f32>();
            da.total_cmp(&db)
        })
        .unwrap();
    let landmarks = LANDMARKS.min(cells);
    let mut landmark_cells: Vec<u32> = Vec::with_capacity(landmarks);
    let mut tables: Vec<Vec<f32>> = Vec::with_capacity(landmarks);
    // Farthest finite cell from everything chosen so far (the seed's
    // Dijkstra bootstraps the first pick).
    let mut nearest_chosen = dijkstra(seed);
    for _ in 0..landmarks {
        let next = (0..cells)
            .filter(|&i| nearest_chosen[i].is_finite())
            .max_by(|&a, &b| nearest_chosen[a].total_cmp(&nearest_chosen[b]))
            .unwrap_or(seed);
        let table = dijkstra(next);
        for i in 0..cells {
            if table[i] < nearest_chosen[i] {
                nearest_chosen[i] = table[i];
            }
        }
        landmark_cells.push(next as u32);
        tables.push(table);
    }
    let mut packed = Vec::with_capacity(cells * landmarks);
    for i in 0..cells {
        for table in &tables {
            packed.push(if table[i].is_finite() {
                ((table[i] / UNIT_LY).ceil() as u32).min(u32::from(UNREACHABLE - 1)) as u16
            } else {
                UNREACHABLE
            });
        }
    }
    Ok(AltOracle { table: packed, landmark_cells, cells })
}

/// Total ordering for non-NaN f32 heap keys.
pub(crate) mod ordered {
    #[derive(PartialEq)]
    pub struct F32(pub f32);
    impl Eq for F32 {}
    impl PartialOrd for F32 {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for F32 {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&other.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import;
    use crate::StarClassCode as _;

    const N: &str = "Neutron Star";

    fn sub_index(stars: &[(f32, f32, f32)]) -> (tempfile::TempDir, Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        let mut source = String::from("[\n");
        for (i, (x, y, z)) in stars.iter().enumerate() {
            source.push_str(&format!(
                "{{\"id64\":{},\"name\":\"H{i}\",\"coords\":{{\"x\":{x},\"y\":{y},\"z\":{z}}},\"bodies\":[{{\"type\":\"Star\",\"subType\":\"{N}\",\"mainStar\":true}}]}}{}\n",
                i + 1,
                if i + 1 == stars.len() { "" } else { "," }
            ));
        }
        source.push(']');
        import::import_reader(Box::new(std::io::Cursor::new(source.into_bytes())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let ndir = dir.path().join("boost250");
        import::subset_cells(&g, &ndir, crate::long_range::NEUTRON_CELL_LY, |r| {
            crate::long_range::highway_star(crate::StarClass::from_code(r.class))
        })
        .unwrap();
        (dir, Galaxy::open(&ndir).unwrap())
    }

    /// The triangle bound never exceeds the true cell-graph distance:
    /// the property that keeps the heuristic honest. Checked for every
    /// cell pair against a reference Dijkstra.
    #[test]
    fn the_bound_never_exceeds_the_true_graph_distance() {
        // A ragged line with a branch, cells 250 ly apart-ish.
        let stars: Vec<(f32, f32, f32)> = (0..12)
            .map(|i| (i as f32 * 260.0, if i % 3 == 0 { 130.0 } else { 0.0 }, 0.0))
            .chain((1..5).map(|i| (1300.0, i as f32 * 260.0, 0.0)))
            .collect();
        let (_dir, sub) = sub_index(&stars);
        let alt = build(&sub, &crate::cgraph::build(&sub).unwrap()).unwrap();
        let cells = sub.cell_count();
        // Reference distances by flooding from every cell.
        for from in 0..cells {
            let truth = reference_dijkstra(&sub, from);
            for goal in 0..cells {
                if !truth[goal].is_finite() {
                    continue;
                }
                let bound = alt.lower_bound_ly(from, goal);
                assert!(
                    bound <= truth[goal] + 1e-3,
                    "bound {bound:.1} exceeds true distance {:.1} for {from} -> {goal}",
                    truth[goal]
                );
            }
        }
    }

    fn reference_dijkstra(sub: &Galaxy, from: usize) -> Vec<f32> {
        let cells = sub.cell_count();
        let cell_ly = sub.cell_ly;
        let centres: Vec<[f32; 3]> = (0..cells).map(|i| centre(sub.cell_entry(i).0, cell_ly)).collect();
        let mut dist = vec![f32::INFINITY; cells];
        dist[from] = 0.0;
        let mut open = vec![from];
        while let Some(i) = open.pop() {
            for j in 0..cells {
                if i == j {
                    continue;
                }
                let d = crate::format::dist(centres[i], centres[j]);
                if d <= R_REF && dist[i] + d < dist[j] - 1e-4 {
                    dist[j] = dist[i] + d;
                    open.push(j);
                }
            }
        }
        dist
    }

    /// A highway that detours around a void: the straight line lies, the
    /// ALT bound does not -- and it is the fixture where the bound must
    /// actually exceed the euclidean distance to be worth anything.
    #[test]
    fn a_void_detour_raises_the_bound_above_the_straight_line() {
        // An L: 4,000 ly along +x, then 4,000 ly along +y. Corner to
        // corner the path is ~8,000 ly, the straight line ~5,657.
        let stars: Vec<(f32, f32, f32)> = (0..=16)
            .map(|i| (i as f32 * 250.0, 0.0, 0.0))
            .chain((1..=16).map(|i| (4000.0, i as f32 * 250.0, 0.0)))
            .collect();
        let (_dir, sub) = sub_index(&stars);
        let alt = build(&sub, &crate::cgraph::build(&sub).unwrap()).unwrap();
        let a = sub.cell_index_of_pos([0.0, 0.0, 0.0]).unwrap();
        let b = sub.cell_index_of_pos([4000.0, 4000.0, 0.0]).unwrap();
        let euclid = crate::format::dist([0.0, 0.0, 0.0], [4000.0, 4000.0, 0.0]);
        let bound = alt.lower_bound_ly(a, b);
        assert!(
            bound > euclid,
            "the detour must show: bound {bound:.0} ly vs straight {euclid:.0} ly"
        );
        assert!(bound <= 8_000.0 + 500.0, "and stay below the true path: {bound:.0}");
    }

    /// The table round-trips through alt250.bin bit for bit and loads
    /// through the Galaxy handle when present. (Not built by the subset
    /// rebuild: measured null on every canonical route -- opt-in via
    /// examples/sidecars.rs.)
    #[test]
    fn the_table_round_trips_and_loads_when_present() {
        let stars: Vec<(f32, f32, f32)> = (0..8).map(|i| (i as f32 * 300.0, 0.0, 0.0)).collect();
        let (dir, sub) = sub_index(&stars);
        let built = build(&sub, &crate::cgraph::build(&sub).unwrap()).unwrap();
        let path = dir.path().join("boost250").join(ALT_FILE);
        built.write(&path).unwrap();
        let reopened = AltOracle::open(&path).unwrap();
        assert_eq!(reopened.cells, built.cells);
        assert_eq!(reopened.landmark_cells, built.landmark_cells);
        assert_eq!(reopened.table, built.table);
        // A fresh handle sees the file; the pre-write handle cached "none".
        let sub = Galaxy::open(&dir.path().join("boost250")).unwrap();
        assert_eq!(sub.alt().map(|a| a.leaf_count()), Some(sub.cell_count()));
    }
}
