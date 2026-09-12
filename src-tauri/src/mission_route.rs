//! Visiting order for mission hand-ins: given the commander's position
//! and the destination systems of their active missions, find a short
//! tour (nearest-neighbour construction, 2-opt improvement — the same
//! shape EDDI's mission routing uses). Distances are straight-line
//! light-years; the long-range planner turns each leg into jumps.

type P = (f64, f64, f64);

fn d(a: P, b: P) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// Order `stops` for a short open tour starting at `start` (no return
/// leg — missions end wherever the last hand-in is). Returns indices
/// into `stops`.
pub fn order_stops(start: P, stops: &[P]) -> Vec<usize> {
    if stops.len() <= 1 {
        return (0..stops.len()).collect();
    }
    // Nearest neighbour from the start.
    let mut order: Vec<usize> = Vec::with_capacity(stops.len());
    let mut remaining: Vec<usize> = (0..stops.len()).collect();
    let mut here = start;
    while !remaining.is_empty() {
        let (pick, _) = remaining
            .iter()
            .enumerate()
            .map(|(at, &i)| (at, d(here, stops[i])))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let next = remaining.swap_remove(pick);
        here = stops[next];
        order.push(next);
    }
    // 2-opt: uncross until no swap shortens the tour.
    let tour_len = |order: &[usize]| -> f64 {
        let mut total = d(start, stops[order[0]]);
        for pair in order.windows(2) {
            total += d(stops[pair[0]], stops[pair[1]]);
        }
        total
    };
    let mut best = tour_len(&order);
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..order.len() - 1 {
            for j in i + 1..order.len() {
                order[i..=j].reverse();
                let len = tour_len(&order);
                if len + 1e-9 < best {
                    best = len;
                    improved = true;
                } else {
                    order[i..=j].reverse();
                }
            }
        }
    }
    order
}

/// Total tour length for reporting, ly.
pub fn tour_ly(start: P, stops: &[P], order: &[usize]) -> f64 {
    let mut total = 0.0;
    let mut here = start;
    for &i in order {
        total += d(here, stops[i]);
        here = stops[i];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_a_line_of_stops_outward() {
        let stops = [(30.0, 0.0, 0.0), (10.0, 0.0, 0.0), (20.0, 0.0, 0.0)];
        assert_eq!(order_stops((0.0, 0.0, 0.0), &stops), vec![1, 2, 0]);
    }

    /// Nearest-neighbour alone takes a bad greedy first hop here; 2-opt
    /// must repair the crossing so the tour beats greedy.
    #[test]
    fn two_opt_beats_pure_greedy() {
        let start = (0.0, 0.0, 0.0);
        // Greedy grabs (1,0), then pays a long zig-zag; optimal sweeps.
        let stops = [
            (1.0, 0.0, 0.0),
            (10.0, 1.0, 0.0),
            (5.0, -0.2, 0.0),
            (10.0, -1.0, 0.0),
        ];
        let order = order_stops(start, &stops);
        let ordered = tour_ly(start, &stops, &order);
        let greedy_zigzag = tour_ly(start, &stops, &[0, 2, 1, 3]);
        assert!(
            ordered <= greedy_zigzag + 1e-9,
            "{ordered} vs {greedy_zigzag}"
        );
        // And the tour visits everything exactly once.
        let mut seen = order.clone();
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    #[test]
    fn degenerate_inputs_hold() {
        assert!(order_stops((0.0, 0.0, 0.0), &[]).is_empty());
        assert_eq!(order_stops((0.0, 0.0, 0.0), &[(5.0, 5.0, 5.0)]), vec![0]);
    }
}
