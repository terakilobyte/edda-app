//! The 100 ly cell grid shared by every sphere query (market search,
//! trade report, stations): mirrors migration 0015's generated column.

/// Grid cell of a point, mirroring migration 0015's generated column
/// EXACTLY (100 ly cubes, +1024 offset per axis, 2048 stride). The
/// searches filter `sy.cell = ANY(<cells covering the box>)` so the
/// planner sees an equality on a column with real statistics instead of
/// three range predicates whose selectivities it multiplies (2026-09-07:
/// 16 estimated vs 6,655 actual stations in a 40 ly sphere).
pub const CELL_LY: f64 = 100.0;

/// The single-point twin of `cells_covering`, kept as the executable
/// statement of the encoding (the tests pin it against the SQL).
#[cfg_attr(not(test), allow(dead_code))]
pub fn cell_of(x: f64, y: f64, z: f64) -> i64 {
    let c = |v: f64| (v / CELL_LY).floor() as i64 + 1024;
    (c(x) * 2048 + c(y)) * 2048 + c(z)
}

/// Every cell touching the axis-aligned box around (ox, oy, oz) ± r —
/// a superset of the sphere, which the exact distance test then trims.
pub fn cells_covering(ox: f64, oy: f64, oz: f64, r: f64) -> Vec<i64> {
    let lo = |v: f64| ((v - r) / CELL_LY).floor() as i64;
    let hi = |v: f64| ((v + r) / CELL_LY).floor() as i64;
    let mut out = Vec::new();
    for cx in lo(ox)..=hi(ox) {
        for cy in lo(oy)..=hi(oy) {
            for cz in lo(oz)..=hi(oz) {
                out.push(((cx + 1024) * 2048 + (cy + 1024)) * 2048 + (cz + 1024));
            }
        }
    }
    out
}

#[cfg(test)]
mod cell_tests {
    use super::*;

    #[test]
    fn the_origin_is_the_pinned_cell() {
        // Same arithmetic as the SQL: ((0+1024)*2048 + 1024)*2048 + 1024.
        assert_eq!(cell_of(0.0, 0.0, 0.0), ((1024 * 2048 + 1024) * 2048) + 1024);
        // Negative coordinates floor toward -inf, as SQL floor() does.
        assert_eq!(cell_of(-0.5, 0.0, 0.0), ((1023 * 2048 + 1024) * 2048) + 1024);
    }

    #[test]
    fn covering_contains_the_origin_and_its_box_corners() {
        let (ox, oy, oz, r) = (122.625, -0.8125, -47.28125, 40.0);
        let cells = cells_covering(ox, oy, oz, r);
        assert!(cells.contains(&cell_of(ox, oy, oz)));
        for (dx, dy, dz) in [(-r, -r, -r), (r, r, r), (-r, r, -r), (r, -r, r)] {
            assert!(cells.contains(&cell_of(ox + dx, oy + dy, oz + dz)), "corner {dx},{dy},{dz}");
        }
        // 40 ly around Deciat: x 82.6..162.6 → cells 0..1, y -40.8..39.2 →
        // -1..0, z -87.3..-7.3 → -1 only → 2 × 2 × 1 = 4.
        assert_eq!(cells.len(), 4);
    }

    #[test]
    fn a_radius_inside_one_cell_is_one_cell() {
        assert_eq!(cells_covering(50.0, 50.0, 50.0, 10.0), vec![cell_of(50.0, 50.0, 50.0)]);
    }

    #[test]
    fn the_maximum_radius_is_a_bounded_list() {
        // 500 ly → 11 cells per axis at most → 1,331 ids: fine for = ANY.
        assert!(cells_covering(0.0, 0.0, 0.0, 500.0).len() <= 11 * 11 * 11);
    }
}
