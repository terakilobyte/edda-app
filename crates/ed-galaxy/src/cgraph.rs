//! `graph250.bin` -- the coarse cell graph, the contraction-hierarchy
//! substrate (ROUTING-NEXT 5.3). The Colonia -> Spase stall showed what
//! the straight-line heuristic cannot know: where the highway has a
//! GAP, the coarse search wanders until its expansion cap, settles, and
//! ships a route worse than the galaxy offers. This graph knows the
//! gaps.
//!
//! **Nodes** are the occupied cells of the boost sub-index, parallel to
//! its morton-ordered cell array. **Highway edges** join cells whose
//! centres are within [`R_REF`] ly, priced in *reference jumps* --
//! centre distance over [`REF_BOOSTED_REACH_LY`], the fastest plausible
//! boosted hop, so the price is a lower bound for every real ship.
//! **Gap edges** keep thin regions honest: a cell with fewer than
//! [`GAP_DEGREE`] highway neighbours also connects to its
//! [`GAP_K`]-nearest cells beyond `R_REF` (within [`R_GAP`]), priced at
//! plain-jump truth -- distance over [`REF_PLAIN_REACH_LY`] -- because
//! crossing a gap is flown on ordinary jumps.
//!
//! **Per plot** one Dijkstra from the goal cell turns the graph into a
//! [`GoalField`]: an exact jumps-to-goal lower bound for every cell,
//! shared by every variant, consulted per expansion as the coarse
//! heuristic's floor. Full CH contraction (millisecond queries with no
//! per-plot Dijkstra) is an optimisation of THIS file, added only if
//! the field's build time ever shows up in a profile.
//!
//! Optional derived data like agg/alt: stored inside the sub-index
//! directory, absence = today's straight-line heuristic.

use std::collections::BinaryHeap;
use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::alt::ordered::F32;
use crate::format::{dist, morton_cell_of, Galaxy};

pub const GRAPH_FILE: &str = "graph250.bin";
const MAGIC: &[u8; 4] = b"GRPH";
/// Highway adjacency radius: at least any ship's boosted reach.
pub const R_REF: f32 = 600.0;
/// Gap edges reach this far past the highway's edge.
pub const R_GAP: f32 = 6_000.0;
/// Cells with fewer highway neighbours than this get gap edges.
pub const GAP_DEGREE: usize = 8;
pub const GAP_K: usize = 4;
/// The fastest plausible boosted hop (Caspian x6) and plain jump: the
/// reference ship that keeps every price a lower bound for real ships.
pub const REF_BOOSTED_REACH_LY: f32 = 470.0;
pub const REF_PLAIN_REACH_LY: f32 = 78.0;
/// Edge costs and field values are stored in 1/64-jump units.
const JUMP_UNIT: f32 = 1.0 / 64.0;
const UNREACHABLE: u16 = u16::MAX;

/// CSR adjacency: `edges[offsets[i]..offsets[i+1]]` are cell `i`'s
/// neighbours as (cell, cost in 1/64 jumps).
pub struct CellGraph {
    offsets: Vec<u32>,
    edges: Vec<(u32, u16)>,
}

impl CellGraph {
    pub fn leaf_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn neighbours(&self, cell: usize) -> &[(u32, u16)] {
        &self.edges[self.offsets[cell] as usize..self.offsets[cell + 1] as usize]
    }

    /// Jumps-to-goal lower bounds for every cell, by one Dijkstra from
    /// the goal cell. ~hundreds of ms on the real 475 k-cell highway;
    /// built once per plot and shared across variants.
    pub fn goal_field(&self, goal_cell: usize) -> GoalField {
        self.goal_field_where(goal_cell, |_| true)
    }

    /// [`CellGraph::goal_field`] restricted to cells `keep` admits (the
    /// endpoints are always admitted). A lone-star cell LOOKS traversable
    /// at cell level while no real chain runs through it -- measured on
    /// Colonia -> Spase, where the unrestricted field lured the frontier
    /// into the x~0 axis desert (thin but occupied) while the actual
    /// highway arc runs 4 kly to the other side. Filtering the field to
    /// chain-viable cells makes its promises realizable by stars.
    pub fn goal_field_where(&self, goal_cell: usize, keep: impl Fn(usize) -> bool) -> GoalField {
        let cells = self.leaf_count();
        let mut best = vec![UNREACHABLE; cells];
        let mut heap: BinaryHeap<std::cmp::Reverse<(F32, u32)>> = BinaryHeap::new();
        best[goal_cell] = 0;
        heap.push(std::cmp::Reverse((F32(0.0), goal_cell as u32)));
        while let Some(std::cmp::Reverse((F32(d), i))) = heap.pop() {
            let du = (d / JUMP_UNIT).round() as u32;
            if du > u32::from(best[i as usize]) {
                continue;
            }
            for &(j, cost) in self.neighbours(i as usize) {
                if !keep(j as usize) {
                    continue;
                }
                let nd = du + u32::from(cost);
                if nd < u32::from(best[j as usize]) && nd < u32::from(UNREACHABLE) {
                    best[j as usize] = nd as u16;
                    heap.push(std::cmp::Reverse((F32(nd as f32 * JUMP_UNIT), j)));
                }
            }
        }
        GoalField { jumps: best }
    }

    /// Shortest path between two cells over the graph, as cell indices
    /// from `from` to `to` inclusive -- A* with the euclidean
    /// reference-jump bound (admissible: every edge costs at least its
    /// distance over [`REF_BOOSTED_REACH_LY`]). `None` when no path
    /// exists even over gap edges.
    pub fn shortest_cell_path(&self, sub: &Galaxy, from: usize, to: usize) -> Option<Vec<u32>> {
        let cells = self.leaf_count();
        debug_assert_eq!(cells, sub.cell_count());
        let centre = |i: usize| -> [f32; 3] {
            let (cx, cy, cz) = morton_cell_of(sub.cell_entry(i).0);
            let cell_ly = sub.cell_ly;
            [
                (cx as f32 + 0.5) * cell_ly,
                (cy as f32 + 0.5) * cell_ly,
                (cz as f32 + 0.5) * cell_ly,
            ]
        };
        let goal_centre = centre(to);
        let h = |i: usize| dist(centre(i), goal_centre) / REF_BOOSTED_REACH_LY;
        let mut best: std::collections::HashMap<u32, f32> = std::collections::HashMap::new();
        let mut parent: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut heap: BinaryHeap<std::cmp::Reverse<(F32, u32)>> = BinaryHeap::new();
        best.insert(from as u32, 0.0);
        heap.push(std::cmp::Reverse((F32(h(from)), from as u32)));
        while let Some(std::cmp::Reverse((F32(f), i))) = heap.pop() {
            let g = f - h(i as usize);
            if g > best.get(&i).copied().unwrap_or(f32::INFINITY) + 1e-3 {
                continue;
            }
            if i as usize == to {
                let mut path = vec![i];
                let mut at = i;
                while let Some(&p) = parent.get(&at) {
                    path.push(p);
                    at = p;
                }
                path.reverse();
                return Some(path);
            }
            for &(j, cost) in self.neighbours(i as usize) {
                let ng = g + cost as f32 * JUMP_UNIT;
                if ng < best.get(&j).copied().unwrap_or(f32::INFINITY) {
                    best.insert(j, ng);
                    parent.insert(j, i);
                    heap.push(std::cmp::Reverse((F32(ng + h(j as usize)), j)));
                }
            }
        }
        None
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let cells = self.leaf_count();
        let mut out = Vec::with_capacity(16 + self.offsets.len() * 4 + self.edges.len() * 6);
        out.extend_from_slice(MAGIC);
        out.push(1u8);
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(&(cells as u32).to_le_bytes());
        out.extend_from_slice(&(self.edges.len() as u32).to_le_bytes());
        for &o in &self.offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        for &(j, cost) in &self.edges {
            out.extend_from_slice(&j.to_le_bytes());
            out.extend_from_slice(&cost.to_le_bytes());
        }
        let mut file =
            std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        file.write_all(&out)?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<CellGraph> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.len() < 16 || &bytes[0..4] != MAGIC {
            bail!("{} is not a GRPH cell graph", path.display());
        }
        ensure!(bytes[4] == 1, "unknown cell-graph version {}", bytes[4]);
        let cells = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let edge_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let need = 16 + (cells + 1) * 4 + edge_count * 6;
        ensure!(
            bytes.len() == need,
            "{} is {} bytes, expected {need}",
            path.display(),
            bytes.len()
        );
        let mut at = 16;
        let mut offsets = Vec::with_capacity(cells + 1);
        for _ in 0..=cells {
            offsets.push(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()));
            at += 4;
        }
        let mut edges = Vec::with_capacity(edge_count);
        for _ in 0..edge_count {
            let j = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let cost = u16::from_le_bytes(bytes[at + 4..at + 6].try_into().unwrap());
            edges.push((j, cost));
            at += 6;
        }
        ensure!(
            offsets.last().copied() == Some(edge_count as u32),
            "{} offsets do not close the edge list",
            path.display()
        );
        Ok(CellGraph { offsets, edges })
    }
}

/// The per-plot product: jumps-to-goal in 1/64-jump units per cell.
pub struct GoalField {
    jumps: Vec<u16>,
}

impl GoalField {
    /// Lower bound in reference jumps from this cell to the goal cell;
    /// `None` when the graph never reaches it (fall back to euclid).
    pub fn jumps_to_goal(&self, cell: usize) -> Option<f32> {
        match self.jumps.get(cell) {
            Some(&UNREACHABLE) | None => None,
            Some(&d) => Some(d as f32 * JUMP_UNIT),
        }
    }

    /// Every cell's jumps-from-source, `INFINITY` where unreached --
    /// the ALT landmark builder consumes whole fields.
    pub fn as_jumps(&self) -> Vec<f32> {
        self.jumps
            .iter()
            .map(|&d| {
                if d == UNREACHABLE {
                    f32::INFINITY
                } else {
                    d as f32 * JUMP_UNIT
                }
            })
            .collect()
    }
}

fn quantise(jumps: f32) -> u16 {
    ((jumps / JUMP_UNIT).ceil() as u32).min(u32::from(UNREACHABLE - 1)) as u16
}

/// The occupied cell nearest a position, by expanding coordinate shells
/// up to `max_ly` -- how a plot endpoint off the highway snaps onto the
/// cell graph.
pub fn nearest_cell(sub: &Galaxy, pos: [f32; 3], max_ly: f32) -> Option<usize> {
    let cell_ly = sub.cell_ly;
    let (cx, cy, cz) = crate::format::cell_of_with(pos, cell_ly);
    let max_ring = (max_ly / cell_ly).ceil() as i32;
    let mut best: Option<(f32, usize)> = None;
    let mut settled_at: Option<i32> = None;
    for ring in 0..=max_ring {
        if let Some(at) = settled_at {
            if ring > at + 1 {
                break;
            }
        }
        for dx in -ring..=ring {
            for dy in -ring..=ring {
                for dz in -ring..=ring {
                    if dx.abs().max(dy.abs()).max(dz.abs()) != ring {
                        continue;
                    }
                    let Some(i) = sub.cell_index(cx + dx, cy + dy, cz + dz) else {
                        continue;
                    };
                    let (mx, my, mz) = morton_cell_of(sub.cell_entry(i).0);
                    let centre = [
                        (mx as f32 + 0.5) * cell_ly,
                        (my as f32 + 0.5) * cell_ly,
                        (mz as f32 + 0.5) * cell_ly,
                    ];
                    let d = dist(pos, centre);
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i));
                    }
                }
            }
        }
        if settled_at.is_none() && best.is_some() {
            settled_at = Some(ring);
        }
    }
    best.map(|(_, i)| i)
}

/// One pass over a morton-ordered (v3) sub-index.
pub fn build(sub: &Galaxy) -> Result<CellGraph> {
    ensure!(
        sub.morton_cells(),
        "the cell graph needs a v3 (morton-ordered) index"
    );
    let cells = sub.cell_count();
    ensure!(cells > 0, "an empty sub-index has no graph");
    let cell_ly = sub.cell_ly;
    let centres: Vec<[f32; 3]> = (0..cells)
        .map(|i| {
            let (cx, cy, cz) = morton_cell_of(sub.cell_entry(i).0);
            [
                (cx as f32 + 0.5) * cell_ly,
                (cy as f32 + 0.5) * cell_ly,
                (cz as f32 + 0.5) * cell_ly,
            ]
        })
        .collect();
    let reach = (R_REF / cell_ly).ceil() as i32;
    // The closest star pair between two cells, early-exiting the moment
    // a pair within the reference boosted reach appears (dense space
    // answers on the first few comparisons). This is what decides
    // whether an edge is a hop or a crossing: center distance within
    // R_REF means nothing when the cells' stars sit at far corners
    // (item 18d -- the Spase-return field descent was priced ~1.3
    // jumps per step over ~780 ly closest pairs, and the frontier
    // chased promises no star chain could keep).
    let min_pair = |i: usize, j: usize| -> f32 {
        let (Some((s0, n0)), Some((s1, n1))) = (sub.cell_records(i), sub.cell_records(j)) else {
            return f32::MAX;
        };
        let mut best = f32::MAX;
        for a in s0..s0 + n0 {
            let pa = sub.pos_of(a);
            for b in s1..s1 + n1 {
                let d = dist(pa, sub.pos_of(b));
                if d < best {
                    best = d;
                    if best <= REF_BOOSTED_REACH_LY {
                        return best;
                    }
                }
            }
        }
        best
    };
    // Highway edges first, so components are known before gap edges.
    // Realizable edges (a star pair within boosted reach) price by
    // center distance as before; star-infeasible ones price as the
    // crossing they are -- one boosted hop plus plain bridging of the
    // closest pair's excess. Both stay lower bounds for every ship
    // (the reference ship is the fastest plausible).
    let mut highway_edges: Vec<Vec<(u32, u16)>> = vec![Vec::new(); cells];
    for i in 0..cells {
        let (cx, cy, cz) = morton_cell_of(sub.cell_entry(i).0);
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    if (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    let Some(j) = sub.cell_index(cx + dx, cy + dy, cz + dz) else {
                        continue;
                    };
                    let d = dist(centres[i], centres[j]);
                    if d <= R_REF {
                        let pair = min_pair(i, j);
                        let jumps = if pair <= REF_BOOSTED_REACH_LY {
                            d / REF_BOOSTED_REACH_LY
                        } else {
                            (d / REF_BOOSTED_REACH_LY)
                                .max(1.0 + (pair - REF_BOOSTED_REACH_LY) / REF_PLAIN_REACH_LY)
                        };
                        highway_edges[i].push((j as u32, quantise(jumps)));
                    }
                }
            }
        }
    }
    // Union-find over the highway: a gap edge is only worth having when
    // it lands in a DIFFERENT component -- the first design took
    // k-nearest beyond R_REF and every edge pointed backward into the
    // cell's own arm (the test caught it), leaving voids uncrossed.
    let mut parent: Vec<u32> = (0..cells as u32).collect();
    fn find(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for i in 0..cells {
        for &(j, _) in &highway_edges[i] {
            let (a, b) = (find(&mut parent, i as u32), find(&mut parent, j));
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let component: Vec<u32> = (0..cells as u32).map(|i| find(&mut parent, i)).collect();
    let max_ring = (R_GAP / cell_ly).ceil() as i32;
    let mut edges_per_cell: Vec<Vec<(u32, u16)>> = highway_edges;
    // Two kinds of gap edge, both priced as plain flying:
    // - FOREIGN-component bridges from thinly connected cells (islands
    //   reachable at all), searched as far as R_GAP;
    // - nearest-beyond-reach edges from EVERY cell, regardless of
    //   component. The Spase lesson (2026-09-01): intra-component
    //   deserts got no edges because the shores share the giant
    //   component the long way round, so the metric priced the desert
    //   as unreachable-or-free. Cheap in dense space (the first shell
    //   past R_REF has cells), far-searching only at true shores.
    for i in 0..cells {
        let low_degree = edges_per_cell[i].len() < GAP_DEGREE;
        let (cx, cy, cz) = morton_cell_of(sub.cell_entry(i).0);
        let mut foreign: Vec<(f32, u32)> = Vec::new();
        let mut beyond: Vec<(f32, u32)> = Vec::new();
        let mut ring = reach + 1;
        let mut foreign_at: Option<i32> = None;
        let mut beyond_at: Option<i32> = None;
        while ring <= max_ring {
            let want_foreign = low_degree && foreign_at.is_none_or(|at| ring <= at + 1);
            let want_beyond = beyond_at.is_none_or(|at| ring <= at + 1);
            if !want_foreign && !want_beyond {
                break;
            }
            for dx in -ring..=ring {
                for dy in -ring..=ring {
                    for dz in -ring..=ring {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != ring {
                            continue;
                        }
                        let Some(j) = sub.cell_index(cx + dx, cy + dy, cz + dz) else {
                            continue;
                        };
                        let d = dist(centres[i], centres[j]);
                        if d <= R_REF {
                            continue; // a highway edge covers it
                        }
                        if want_beyond {
                            beyond.push((d, j as u32));
                        }
                        if want_foreign && component[j] != component[i] {
                            foreign.push((d, j as u32));
                        }
                    }
                }
            }
            if foreign_at.is_none() && !foreign.is_empty() {
                foreign_at = Some(ring);
            }
            if beyond_at.is_none() && !beyond.is_empty() {
                beyond_at = Some(ring);
            }
            ring += 1;
        }
        foreign.sort_by(|a, b| a.0.total_cmp(&b.0));
        beyond.sort_by(|a, b| a.0.total_cmp(&b.0));
        for &(d, j) in beyond.iter().take(GAP_K).chain(foreign.iter().take(GAP_K)) {
            if d <= R_GAP {
                edges_per_cell[i].push((j, quantise(d / REF_PLAIN_REACH_LY)));
            }
        }
    }
    // Symmetrise: a gap edge must be crossable both ways or the goal
    // field cannot flow back over it (Dijkstra runs FROM the goal).
    let mut symmetric: Vec<Vec<(u32, u16)>> = vec![Vec::new(); cells];
    for (i, list) in edges_per_cell.iter().enumerate() {
        for &(j, cost) in list {
            symmetric[i].push((j, cost));
            symmetric[j as usize].push((i as u32, cost));
        }
    }
    let mut offsets = Vec::with_capacity(cells + 1);
    let mut flat: Vec<(u32, u16)> = Vec::new();
    offsets.push(0u32);
    for list in &mut symmetric {
        list.sort_unstable();
        list.dedup_by_key(|e| e.0);
        flat.extend_from_slice(list);
        offsets.push(flat.len() as u32);
    }
    Ok(CellGraph {
        offsets,
        edges: flat,
    })
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
        import::import_reader(
            Box::new(std::io::Cursor::new(source.into_bytes())),
            dir.path(),
            &mut |_| {},
        )
        .unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let ndir = dir.path().join("boost250");
        import::subset_cells(&g, &ndir, crate::long_range::NEUTRON_CELL_LY, |r| {
            crate::long_range::highway_star(crate::StarClass::from_code(r.class))
        })
        .unwrap();
        (dir, Galaxy::open(&ndir).unwrap())
    }

    /// Two arm clusters separated by a 3,000 ly void: highway edges knit
    /// each arm, gap edges bridge the void at plain-jump prices, and the
    /// goal field prices the crossing honestly -- the number the
    /// straight-line heuristic can never know.
    #[test]
    fn gap_edges_bridge_a_void_and_the_field_prices_the_crossing() {
        let mut stars: Vec<(f32, f32, f32)> =
            (0..8).map(|i| (i as f32 * 300.0, 0.0, 0.0)).collect();
        stars.extend((0..8).map(|i| (5_100.0 + i as f32 * 300.0, 0.0, 0.0)));
        let (_dir, sub) = sub_index(&stars);
        let graph = build(&sub).unwrap();
        assert_eq!(graph.leaf_count(), sub.cell_count());
        let west_end = sub.cell_index_of_pos([2_100.0, 0.0, 0.0]).unwrap();
        let east_start = sub.cell_index_of_pos([5_100.0, 0.0, 0.0]).unwrap();
        assert!(
            graph
                .neighbours(west_end)
                .iter()
                .any(|&(j, _)| j as usize == east_start),
            "the void's western shore must hold a gap edge east"
        );
        let goal = sub.cell_index_of_pos([7_200.0, 0.0, 0.0]).unwrap();
        let field = graph.goal_field(goal);
        // From the far west end: ~2,100 ly of highway (boosted prices)
        // plus a ~3,000 ly gap at plain prices (~38 jumps) plus the east
        // arm. The crossing must dominate, and the bound must stay under
        // the true cost for ANY ship (reference = fastest).
        let west = sub.cell_index_of_pos([0.0, 0.0, 0.0]).unwrap();
        let bound = field
            .jumps_to_goal(west)
            .expect("west arm reaches the goal through the gap");
        assert!(
            bound > 30.0,
            "the plain crossing must dominate the price: {bound:.1} jumps"
        );
        assert!(bound < 60.0, "and stay a lower bound: {bound:.1} jumps");
        // Inside the goal arm the price is a handful of boosted hops.
        let near = sub.cell_index_of_pos([6_600.0, 0.0, 0.0]).unwrap();
        assert!(field.jumps_to_goal(near).unwrap() < 4.0);
        // The goal itself is free; unreachable stays None.
        assert_eq!(field.jumps_to_goal(goal), Some(0.0));
    }

    /// The path query crosses the void the gap edges bridged, and the
    /// endpoint snap finds the nearest occupied cell from off-highway.
    #[test]
    fn the_cell_path_crosses_the_void_end_to_end() {
        let mut stars: Vec<(f32, f32, f32)> =
            (0..8).map(|i| (i as f32 * 300.0, 0.0, 0.0)).collect();
        stars.extend((0..8).map(|i| (5_100.0 + i as f32 * 300.0, 0.0, 0.0)));
        let (_dir, sub) = sub_index(&stars);
        let graph = build(&sub).unwrap();
        let from =
            nearest_cell(&sub, [-40.0, 60.0, 0.0], 2_000.0).expect("snaps near the west end");
        let to = nearest_cell(&sub, [7_300.0, 0.0, 0.0], 2_000.0).expect("snaps near the east end");
        let path = graph
            .shortest_cell_path(&sub, from, to)
            .expect("the gap edges make it reachable");
        assert_eq!(path.first(), Some(&(from as u32)));
        assert_eq!(path.last(), Some(&(to as u32)));
        // Monotonic eastward along the line: the path never doubles back.
        let xs: Vec<f32> = path
            .iter()
            .map(|&i| {
                let (cx, _, _) = crate::format::morton_cell_of(sub.cell_entry(i as usize).0);
                (cx as f32 + 0.5) * sub.cell_ly
            })
            .collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "path wanders: {xs:?}");
        assert!(
            nearest_cell(&sub, [50_000.0, 0.0, 0.0], 1_000.0).is_none(),
            "far off everything snaps to nothing"
        );
    }

    /// A highway edge's price must reflect the hop a ship can actually
    /// fly: cell centers within R_REF mean nothing when the cells' only
    /// stars sit at far corners (item 18d -- the Spase-return field
    /// descent priced ~1.3 jumps per step over steps whose closest star
    /// pair was ~780 ly, and the frontier chased promises no chain
    /// could keep).
    #[test]
    fn a_star_infeasible_highway_edge_prices_as_its_real_crossing() {
        let (_dir, sub) = sub_index(&[(5.0, 0.0, 0.0), (745.0, 0.0, 0.0)]);
        let graph = build(&sub).unwrap();
        let a = sub.cell_index_of_pos([5.0, 0.0, 0.0]).unwrap();
        let b = sub.cell_index_of_pos([745.0, 0.0, 0.0]).unwrap();
        let cost = graph
            .neighbours(a)
            .iter()
            .find(|&&(j, _)| j as usize == b)
            .map(|&(_, c)| f32::from(c) / 64.0)
            .expect("centers are within R_REF; the edge exists");
        // 740 ly with no reachable star pair: one boosted hop (470 ly)
        // plus ~270 ly of plain bridging (~3.5 jumps at 78 ly) -- not
        // the ~1.06 the center distance alone would suggest.
        assert!(
            cost >= 4.0,
            "the edge must price its real crossing: {cost:.2} jumps"
        );
        assert!(
            cost <= 6.0,
            "and stay a lower bound for any ship: {cost:.2} jumps"
        );
    }

    /// Highway edges price at boosted reach, and the file round-trips.
    #[test]
    fn edge_prices_and_the_file_round_trip() {
        let stars: Vec<(f32, f32, f32)> = (0..4).map(|i| (i as f32 * 300.0, 0.0, 0.0)).collect();
        let (dir, sub) = sub_index(&stars);
        let graph = build(&sub).unwrap();
        let a = sub.cell_index_of_pos([0.0, 0.0, 0.0]).unwrap();
        let b = sub.cell_index_of_pos([300.0, 0.0, 0.0]).unwrap();
        let (_, cost) = *graph
            .neighbours(a)
            .iter()
            .find(|&&(j, _)| j as usize == b)
            .unwrap();
        let expected = (250.0f32 / REF_BOOSTED_REACH_LY / JUMP_UNIT).ceil() as u16;
        assert_eq!(
            cost, expected,
            "a 250 ly highway hop prices as boosted reach"
        );
        let path = dir.path().join("boost250").join(GRAPH_FILE);
        graph.write(&path).unwrap();
        let reopened = CellGraph::open(&path).unwrap();
        assert_eq!(reopened.leaf_count(), graph.leaf_count());
        assert_eq!(reopened.edge_count(), graph.edge_count());
        assert_eq!(reopened.neighbours(a), graph.neighbours(a));
        let fresh = Galaxy::open(&dir.path().join("boost250")).unwrap();
        assert_eq!(
            fresh.cell_graph().map(|g| g.leaf_count()),
            Some(fresh.cell_count())
        );
    }
}
