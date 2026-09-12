//! `agg250.bin` -- prefix aggregates over the boost sub-index's morton
//! cells, per the ROUTING-NEXT item 5.4 spec: an O(log n) "is there any
//! scoopable / highway star in this volume" oracle for the min-fuel
//! planner's per-expansion pruning and the trap guard's multi-jump
//! reachability. The walked cell scan is already sub-millisecond per
//! corridor; the aggregate pays where a search asks thousands of times
//! per plot.
//!
//! Optional derived data, like the sub-index itself: built in one pass
//! over the morton-sorted cells at highway-rebuild time and stored
//! INSIDE the sub-index directory (the spec said "beside"; inside, the
//! background rebuild's staging-dir swap and deletion cover it with no
//! extra handling -- flagged to the spec's author). Absence = off.
//!
//! Layout (`AGG1`): levels from the leaves up, three bits of morton key
//! per step. The leaf level is one 4-byte node per occupied cell,
//! PARALLEL to the sub-index's own morton-ordered cell array -- v3 made
//! the orders identical, so leaves store no keys at all. Each upper
//! level stores its nodes plus one u32 `first_leaf` per node (the index
//! of its subtree's first leaf), which is all descent needs: a node's
//! leaf range comes from its neighbour's `first_leaf`, and its children
//! are the nodes of the level below whose `first_leaf` falls inside it.
//! Node = u8 flags, u8 best boost class code, u16 star count
//! (saturating).

use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::format::{dist, morton_cell_of, Galaxy, FLAG_SCOOP_NEARBY};
use crate::star::{StarClass, StarClassCode as _};

/// File name inside the sub-index directory.
pub const AGG_FILE: &str = "agg250.bin";
const MAGIC: &[u8; 4] = b"AGG1";

/// A star that can refuel the ship on arrival: a scoopable main star, or
/// the scoop-nearby flag (a scoopable companion close to the arrival
/// point). In a highway sub-index the classes are neutrons and white
/// dwarfs, so this is in practice the refuelling-neutron bit.
pub const ANY_SCOOPABLE: u8 = 1 << 0;
pub const ANY_NEUTRON: u8 = 1 << 1;
pub const ANY_WHITE_DWARF: u8 = 1 << 2;
/// Reserved for the populated overlay (stations for scoopless ships);
/// never set by [`build`] today, so never ask for it.
pub const ANY_STATION: u8 = 1 << 3;

/// One aggregate node: what its whole subtree holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub flags: u8,
    /// Class code of the best supercharge on offer in the subtree
    /// (neutron beats white dwarf on every drive), `StarClass::Unknown`'s
    /// code when neither is present.
    pub best_boost_class: u8,
    /// Stars in the subtree, saturating at `u16::MAX`.
    pub count: u16,
}

struct Level {
    /// Bits of morton key dropped at this level (0 at the leaves).
    shift: u32,
    nodes: Vec<Node>,
    /// Index of each node's first leaf; empty at the leaf level, where
    /// node `i` IS leaf `i`.
    first_leaf: Vec<u32>,
}

pub struct Aggregate {
    /// `levels[0]` is the leaves; the last level has a single root.
    levels: Vec<Level>,
}

impl Aggregate {
    /// Leaves = the sub-index's occupied cells, in the same order. An
    /// aggregate only answers for the index it was built from.
    pub fn leaf_count(&self) -> usize {
        self.levels[0].nodes.len()
    }
}

/// What a query cost, for the pins and the bench.
#[derive(Clone, Copy, Debug, Default)]
pub struct Probe {
    pub nodes_visited: u32,
    /// Deepest level entered, root = 1 (a pruned root never reaches 2).
    pub depth: u32,
    /// Star records actually scanned in leaf cells.
    pub records_scanned: u32,
}

fn star_flags(g: &Galaxy, idx: u32) -> u8 {
    let code = g.class_code(idx);
    let class = if code == StarClass::Unknown.code() {
        g.class(&g.record(idx))
    } else {
        StarClass::from_code(code)
    };
    let mut flags = 0u8;
    if class == StarClass::Neutron {
        flags |= ANY_NEUTRON;
    }
    if class == StarClass::WhiteDwarf {
        flags |= ANY_WHITE_DWARF;
    }
    if class.scoopable() || g.flags(idx) & FLAG_SCOOP_NEARBY != 0 {
        flags |= ANY_SCOOPABLE;
    }
    flags
}

fn best_boost_class(flags: u8) -> u8 {
    if flags & ANY_NEUTRON != 0 {
        StarClass::Neutron.code()
    } else if flags & ANY_WHITE_DWARF != 0 {
        StarClass::WhiteDwarf.code()
    } else {
        StarClass::Unknown.code()
    }
}

/// One pass over a morton-sorted (v3) sub-index.
pub fn build(sub: &Galaxy) -> Result<Aggregate> {
    ensure!(
        sub.morton_cells(),
        "aggregates need a v3 (morton-ordered) index; rebuild the sub-index first"
    );
    let cells = sub.cell_count();
    let mut leaves = Vec::with_capacity(cells);
    for i in 0..cells {
        let (_, start, count) = sub.cell_entry(i);
        let mut node = Node::default();
        for r in start..start + count {
            node.flags |= star_flags(sub, r);
        }
        node.best_boost_class = best_boost_class(node.flags);
        node.count = count.min(u32::from(u16::MAX)) as u16;
        leaves.push(node);
    }
    let mut levels = vec![Level {
        shift: 0,
        nodes: leaves,
        first_leaf: Vec::new(),
    }];
    // Group the leaves by ever-shorter prefixes until one root remains.
    // The morton sort makes each group a contiguous leaf run.
    let mut shift = 3u32;
    while levels.last().unwrap().nodes.len() > 1 {
        ensure!(shift < 64, "morton prefixes exhausted before a single root");
        let mut nodes: Vec<Node> = Vec::new();
        let mut first_leaf: Vec<u32> = Vec::new();
        let mut previous: Option<u64> = None;
        for i in 0..cells {
            let prefix = sub.cell_entry(i).0 >> shift;
            if previous != Some(prefix) {
                previous = Some(prefix);
                nodes.push(Node::default());
                first_leaf.push(i as u32);
            }
            let leaf = levels[0].nodes[i];
            let node = nodes.last_mut().unwrap();
            node.flags |= leaf.flags;
            node.count = node.count.saturating_add(leaf.count);
        }
        for node in &mut nodes {
            node.best_boost_class = best_boost_class(node.flags);
        }
        levels.push(Level {
            shift,
            nodes,
            first_leaf,
        });
        shift += 3;
    }
    Ok(Aggregate { levels })
}

impl Aggregate {
    /// Is any star matching one of `want`'s bits within `radius` of
    /// `center` -- or, with `want == 0`, any star at all? Exact:
    /// internal nodes prune by flags and by their aligned prefix cube,
    /// leaf hits fall through to the sub-index's own records. Agrees
    /// with a brute-force scan by construction.
    pub fn any_within(&self, sub: &Galaxy, center: [f32; 3], radius: f32, want: u8) -> bool {
        self.any_within_probed(sub, center, radius, want).0
    }

    pub fn any_within_probed(
        &self,
        sub: &Galaxy,
        center: [f32; 3],
        radius: f32,
        want: u8,
    ) -> (bool, Probe) {
        let mut probe = Probe::default();
        let top = self.levels.len() - 1;
        debug_assert_eq!(
            self.levels[0].nodes.len(),
            sub.cell_count(),
            "aggregate belongs to another sub-index"
        );
        if self.levels[0].nodes.is_empty() {
            return (false, probe);
        }
        let r2 = radius * radius;
        let cell_ly = sub.cell_ly;
        // (level, node index) pairs still to look at.
        let mut stack: Vec<(usize, usize)> = (0..self.levels[top].nodes.len())
            .map(|i| (top, i))
            .collect();
        while let Some((li, ni)) = stack.pop() {
            probe.nodes_visited += 1;
            probe.depth = probe.depth.max((self.levels.len() - li) as u32);
            let level = &self.levels[li];
            let node = level.nodes[ni];
            if want != 0 && node.flags & want == 0 {
                continue;
            }
            let (leaf_lo, leaf_hi) = self.leaf_range(li, ni);
            // The node's aligned prefix cube: the subtree's smallest key
            // with the dropped bits zeroed is its low corner cell, and
            // the cube is 2^(shift/3) cells on a side.
            let min_key = (sub.cell_entry(leaf_lo as usize).0 >> level.shift) << level.shift;
            let (cx, cy, cz) = morton_cell_of(min_key);
            let side = (1u32 << (level.shift / 3)) as f32 * cell_ly;
            let gap = |lo: f32, p: f32| -> f32 {
                if p < lo {
                    lo - p
                } else if p > lo + side {
                    p - (lo + side)
                } else {
                    0.0
                }
            };
            let (gx, gy, gz) = (
                gap(cx as f32 * cell_ly, center[0]),
                gap(cy as f32 * cell_ly, center[1]),
                gap(cz as f32 * cell_ly, center[2]),
            );
            if gx * gx + gy * gy + gz * gz > r2 {
                continue;
            }
            if li == 0 {
                // A leaf whose cell touches the sphere: the answer is in
                // its actual records.
                let (_, start, count) = sub.cell_entry(ni);
                for r in start..start + count {
                    probe.records_scanned += 1;
                    if (want == 0 || star_flags(sub, r) & want != 0)
                        && dist(sub.pos_of(r), center) <= radius
                    {
                        return (true, probe);
                    }
                }
                continue;
            }
            // Children: the nodes of the level below whose first_leaf
            // falls inside this node's leaf range (leaf level: the
            // leaves themselves).
            let below = &self.levels[li - 1];
            if li == 1 {
                for child in leaf_lo..leaf_hi {
                    stack.push((0, child as usize));
                }
            } else {
                let from = below.first_leaf.partition_point(|&f| f < leaf_lo);
                let to = below.first_leaf.partition_point(|&f| f < leaf_hi);
                for child in from..to {
                    stack.push((li - 1, child));
                }
            }
        }
        (false, probe)
    }

    fn leaf_range(&self, li: usize, ni: usize) -> (u32, u32) {
        if li == 0 {
            return (ni as u32, ni as u32 + 1);
        }
        let level = &self.levels[li];
        let lo = level.first_leaf[ni];
        let hi = level
            .first_leaf
            .get(ni + 1)
            .copied()
            .unwrap_or(self.levels[0].nodes.len() as u32);
        (lo, hi)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(1u8); // format version
        out.push(ANY_SCOOPABLE | ANY_NEUTRON | ANY_WHITE_DWARF); // flags this build fills
        out.extend_from_slice(&[0u8; 2]);
        out.extend_from_slice(&(self.levels.len() as u32).to_le_bytes());
        for level in &self.levels {
            out.extend_from_slice(&level.shift.to_le_bytes());
            out.extend_from_slice(&(level.nodes.len() as u32).to_le_bytes());
        }
        for level in &self.levels {
            for node in &level.nodes {
                out.push(node.flags);
                out.push(node.best_boost_class);
                out.extend_from_slice(&node.count.to_le_bytes());
            }
            for &f in &level.first_leaf {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        let mut file =
            std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        file.write_all(&out)?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Aggregate> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let take = |from: usize, len: usize| -> Result<&[u8]> {
            bytes
                .get(from..from + len)
                .with_context(|| format!("{} is truncated at byte {from}", path.display()))
        };
        if take(0, 4)? != MAGIC {
            bail!("{} is not an AGG1 aggregate", path.display());
        }
        ensure!(bytes[4] == 1, "unknown aggregate version {}", bytes[4]);
        let level_count = u32::from_le_bytes(take(8, 4)?.try_into().unwrap()) as usize;
        ensure!(
            (1..=22).contains(&level_count),
            "implausible level count {level_count}"
        );
        let mut shapes = Vec::with_capacity(level_count);
        let mut at = 12;
        for _ in 0..level_count {
            let shift = u32::from_le_bytes(take(at, 4)?.try_into().unwrap());
            let count = u32::from_le_bytes(take(at + 4, 4)?.try_into().unwrap()) as usize;
            shapes.push((shift, count));
            at += 8;
        }
        let mut levels = Vec::with_capacity(level_count);
        for (index, (shift, count)) in shapes.into_iter().enumerate() {
            let mut nodes = Vec::with_capacity(count);
            for _ in 0..count {
                let b = take(at, 4)?;
                nodes.push(Node {
                    flags: b[0],
                    best_boost_class: b[1],
                    count: u16::from_le_bytes(b[2..4].try_into().unwrap()),
                });
                at += 4;
            }
            let mut first_leaf = Vec::new();
            if index > 0 {
                first_leaf.reserve(count);
                for _ in 0..count {
                    first_leaf.push(u32::from_le_bytes(take(at, 4)?.try_into().unwrap()));
                    at += 4;
                }
            }
            levels.push(Level {
                shift,
                nodes,
                first_leaf,
            });
        }
        ensure!(
            at == bytes.len(),
            "{} has {} trailing bytes",
            path.display(),
            bytes.len() - at
        );
        Ok(Aggregate { levels })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import;

    const N: &str = "Neutron Star";
    const D: &str = "White Dwarf (DA) Star";
    const K: &str = "K (Yellow-Orange) Star";

    /// A v3 index + sub-index over a handful of highway stars spread far
    /// apart (250 ly cells), one with a scoopable companion nearby.
    fn field(stars: &[(&str, f32, f32, f32, &str)]) -> (tempfile::TempDir, Galaxy, Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        let mut source = String::from("[\n");
        for (i, (name, x, y, z, sub)) in stars.iter().enumerate() {
            source.push_str(&format!(
                "{{\"id64\":{},\"name\":\"{name}\",\"coords\":{{\"x\":{x},\"y\":{y},\"z\":{z}}},\"bodies\":[{{\"type\":\"Star\",\"subType\":\"{sub}\",\"mainStar\":true}}]}}{}\n",
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
        let sub = Galaxy::open(&ndir).unwrap();
        (dir, g, sub)
    }

    fn brute_force(sub: &Galaxy, center: [f32; 3], radius: f32, want: u8) -> bool {
        let mut hit = false;
        sub.for_each_within(center, radius, |idx, _| {
            hit |= star_flags(sub, idx) & want != 0
        });
        hit
    }

    /// Item 33 groundwork: the aggregate built over the MAIN index (not
    /// the highway sub-index) answers "is there a scoopable star within
    /// this radius?" — the reserve floor's bail-out question. The K star
    /// is scoopable, the neutron is not; a sphere around the lonely
    /// neutron finds nothing scoopable until it reaches the K.
    #[test]
    fn the_full_index_answers_scoopable_within() {
        let (_t, g, _sub) = field(&[
            ("Farmer", 0.0, 0.0, 0.0, K),
            ("Lonely", 900.0, 0.0, 0.0, N),
            ("Beacon", 1800.0, 0.0, 0.0, N),
        ]);
        let agg = build(&g).expect("main index is v3 morton, so the aggregate builds over it");
        let at = |x: f32| [x, 0.0, 0.0];
        assert!(
            agg.any_within(&g, at(0.0), 50.0, ANY_SCOOPABLE),
            "the K star is scoopable"
        );
        assert!(
            !agg.any_within(&g, at(900.0), 500.0, ANY_SCOOPABLE),
            "nothing scoopable near the lonely neutron"
        );
        assert!(
            agg.any_within(&g, at(900.0), 950.0, ANY_SCOOPABLE),
            "wide enough to reach the K"
        );
        assert!(
            agg.any_within(&g, at(900.0), 950.0, ANY_NEUTRON),
            "the neutron itself is seen too"
        );
    }

    /// The oracle agrees with a brute-force scan on 1,000 random spheres
    /// over a sparse highway field -- the property that makes it usable
    /// as a pruning bound. Random via xorshift, seeded, no dependency.
    #[test]
    fn oracle_agrees_with_brute_force_on_random_spheres() {
        let stars: Vec<(String, f32, f32, f32, &str)> = (0..40)
            .map(|i| {
                let mut s = 0x9e3779b97f4a7c15u64.wrapping_mul(i as u64 + 1);
                let mut next = move || {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    (s % 12_000) as f32 - 6_000.0
                };
                (
                    format!("H{i}"),
                    next(),
                    next(),
                    next(),
                    if i % 3 == 0 { D } else { N },
                )
            })
            .collect();
        let refs: Vec<(&str, f32, f32, f32, &str)> = stars
            .iter()
            .map(|(n, x, y, z, s)| (n.as_str(), *x, *y, *z, *s))
            .collect();
        let (_dir, _g, sub) = field(&refs);
        let agg = build(&sub).unwrap();
        let mut s = 0xdeadbeefcafef00du64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for i in 0..1_000 {
            let center = [
                (next() % 16_000) as f32 - 8_000.0,
                (next() % 16_000) as f32 - 8_000.0,
                (next() % 16_000) as f32 - 8_000.0,
            ];
            let radius = (next() % 3_000) as f32 + 10.0;
            // 0 = any star at all (flags never prune, geometry still does).
            let any_star = {
                let mut hit = false;
                sub.for_each_within(center, radius, |_, _| hit = true);
                hit
            };
            assert_eq!(
                agg.any_within(&sub, center, radius, 0),
                any_star,
                "sphere {i} any-star diverged"
            );
            for want in [
                ANY_NEUTRON,
                ANY_WHITE_DWARF,
                ANY_SCOOPABLE,
                ANY_NEUTRON | ANY_WHITE_DWARF,
            ] {
                assert_eq!(
                    agg.any_within(&sub, center, radius, want),
                    brute_force(&sub, center, radius, want),
                    "sphere {i} at {center:?} r {radius} want {want:#04b} diverged"
                );
            }
        }
    }

    /// A fuel-dark volume dies near the root: asking for a scoopable in a
    /// sphere with none costs a handful of node reads, no record scans.
    #[test]
    fn a_fuel_dark_sphere_returns_false_at_depth_three_or_less() {
        let (_dir, _g, sub) = field(&[
            ("N0", 0.0, 0.0, 0.0, N),
            ("N1", 300.0, 0.0, 0.0, N),
            ("N2", 600.0, 0.0, 0.0, N),
        ]);
        let agg = build(&sub).unwrap();
        // Far off the highway line: no scoopable (none exists at all --
        // plain neutrons refuel nothing).
        let (hit, probe) =
            agg.any_within_probed(&sub, [5_000.0, 5_000.0, 5_000.0], 900.0, ANY_SCOOPABLE);
        assert!(!hit);
        assert!(probe.depth <= 3, "took {} levels", probe.depth);
        assert_eq!(
            probe.records_scanned, 0,
            "flags alone must answer a dark volume"
        );
    }

    /// The highway rebuild writes the oracle beside the cells it
    /// aggregates: a fresh sub-index directory carries a working
    /// agg250.bin without anyone asking.
    #[test]
    fn a_subset_build_writes_the_aggregate_with_the_sub_index() {
        let (dir, _g, sub) = field(&[("N0", 0.0, 0.0, 0.0, N), ("N1", 400.0, 0.0, 0.0, N)]);
        let agg = Aggregate::open(&dir.path().join("boost250").join(AGG_FILE)).unwrap();
        assert!(agg.any_within(&sub, [0.0, 0.0, 0.0], 50.0, ANY_NEUTRON));
    }

    /// Node payloads aggregate what the leaves hold, and the round trip
    /// through agg250.bin preserves every level bit for bit.
    #[test]
    fn payloads_aggregate_and_round_trip_through_the_file() {
        let (_dir, _g, sub) = field(&[
            ("N0", 0.0, 0.0, 0.0, N),
            ("D0", 260.0, 0.0, 0.0, D),
            ("K0", 520.0, 0.0, 0.0, K),
        ]);
        // The sub-index keeps highway stars only: the K star is not there.
        assert_eq!(sub.cell_count(), 2);
        let agg = build(&sub).unwrap();
        let root = agg.levels.last().unwrap().nodes[0];
        assert_eq!(root.flags, ANY_NEUTRON | ANY_WHITE_DWARF);
        assert_eq!(
            root.best_boost_class,
            crate::StarClass::Neutron.code(),
            "neutron beats white dwarf"
        );
        assert_eq!(root.count, 2);
        let path = _dir.path().join(AGG_FILE);
        agg.write(&path).unwrap();
        let reopened = Aggregate::open(&path).unwrap();
        assert_eq!(reopened.levels.len(), agg.levels.len());
        for (a, b) in agg.levels.iter().zip(reopened.levels.iter()) {
            assert_eq!(a.shift, b.shift);
            assert_eq!(a.nodes, b.nodes);
            assert_eq!(a.first_leaf, b.first_leaf);
        }
        // And the reopened oracle still answers.
        assert!(reopened.any_within(&sub, [0.0, 0.0, 0.0], 50.0, ANY_NEUTRON));
        assert!(!reopened.any_within(&sub, [0.0, 0.0, 0.0], 50.0, ANY_WHITE_DWARF));
    }
}
