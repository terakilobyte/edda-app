# Bench archive

Every Atlas matrix sweep lands here as a dated CSV with a metadata
header (what build, what comparator, what index, which directions).
The point is diffability across eras: when a planner change re-pins
the matrix, the old pins stay on the record instead of living in one
session's scratchpad.

Rules:
- One CSV per sweep, named `YYYY-MM-DD-sweepN-<what-changed>.csv`.
- The header comments carry build/commit context, ship definitions,
  budgets, and any hand-edited rows (say WHY — e.g. a cell skipped
  around a known bug).
- Wall times are wall (the parenthesised value in bench's output, not
  the winner's elapsed) — a regex against the wrong column cost us a
  full re-sweep once.
- The bench harness itself: `cargo run -p ed-galaxy --example bench
  --release -- .data/galaxy <from> <to> [--ship s] [--budget s]`.
  Sweep scripts live in the session scratchpad; promote one here if it
  stabilises.

Eras so far (2026-09-01, the goal-field night):
1. **sweep2** — pre-goal-field baseline: Colonia→Spase E at 284 j /
   24 s hugging the desert line.
2. **sweep3** — goal field + chain-viability floor: 91 j / ~1 s,
   Spansh's ×4 optimum rescaled to ×6. Healthy cells byte-identical.
3. **sweep4** — grace clock (14a) + pilot-time comparator (17,
   stop = 3.0 jumps): stop-heavy routes lose to slightly longer clean
   ones (Wongi→Colonia 57 j/10 stops → 58 j/6 stops).
4. **sweep5** — the same build, both directions: the direction
   asymmetry on the record (Beagle→Wongi 196 j / 0.8 s vs
   Wongi→Beagle 190–215 j / 20–54 s).
