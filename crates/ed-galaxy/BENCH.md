# Plotter regression baseline

Explorer Mk II model (size 8A SCO Mk II, 128 t, size 5 booster), full tank,
`cargo run -p ed-galaxy --example bench --release -- .data/galaxy Wongi <to>`.
Re-run before changing the planner; fewer jumps or same jumps faster is a
win, more jumps is a regression to explain.

| route | jumps | scoop stops | time | date |
|---|---|---|---|---|
| Wongi → Colonia | 58 | 9 | 5.8 s | 2026-08-28 (companion index) |
| Colonia → Wongi | 54 | 6 | 2.1 s | 2026-08-28 (Spansh exact plotter: 54 with refuel-every-scoopable, 58 plain) |
| Wongi → Sagittarius A* | 67 | 7 | 2.4 s | 2026-08-28 |
| Wongi → Beagle Point | 190 | 40 | 120 s (waves) | 2026-08-28 (Spansh exact plotter: 208; 226 before bridged legs counted as refuelled) |
| Colonia → Wongi, Mandalay (5A SCO ×4, 32 t) | 86 | 23 | 2.0 s | 2026-08-28 (Spansh: 93 in 120 s) |

Before the companion flag existed in the index (2026-08-28 morning):
55 / 60 / 70 / 222 / 100 -- every refuel was a detour off the highway.

Those are `--thorough --budget 120`, what the app runs (effort "high").
Waves: when a variant's coarse search settles on its expansion allowance
short of the goal and the budget has room, the whole portfolio runs again
with 4× the allowance (then 16×, 64×) until a wave stops improving or the
deadline; routes whose coarse search reaches the goal (Colonia) finish
after the first wave.
Without `--thorough` the thinned variants alone give 57 / 60 / 70 / 244 / 102
in 2–6 s.

How the time goes (2026-08-28, after the Mandalay took 32 s): the coarse
search relaxed every neutron in reach (~3,000 per expansion in the core);
now candidates are thinned to the most forward and the nearest per
~55 ly cell (`bucket_f`, a portfolio axis: 0.5, 0.75, and unthinned in
thorough), and the exact leg planner relaxes at most `LEG_FANOUT` (512)
stars per expansion, scoopables ranked a jump ahead so no fuel stop is
thinned away. Two correctness fixes came out of the same profile: a hop
whose fuel was charged at the boosted rate is flagged boosted even when
the distance alone would not need it (the re-simulation used to fly it
unboosted and fail the leg), and the goal edge no longer needs the tank
to fund a whole bridge (a 32 t ship stalled 600 ly short for the entire
expansion budget). The portfolio runs against `RouteRequest::time_budget_ms`;
Stop keeps the best variant that finished.

Safety: every hop is planned as if the tank were 10 t heavier than scheduled
(`fuel::FUEL_HEADROOM_T_DEFAULT`), capped at a full tank, plus 0.3 ly of slack.
A percentage margin (1.5 %) was tried first and cost 16 jumps on Colonia → Wongi
because a full tank cannot take the natural 430–440 ly highway hops with it.

History: before 2026-08-28 Colonia did not finish in 25 minutes. A version
flown to Colonia (no margin at all) plotted 72 / 81 / 269 and a hop planned
at the edge of reach failed in game with 4 t more fuel than planned.

Companion scooping: `FLAG_SCOOP_NEARBY` is set at import for systems with a
scoopable star within 1,500 ls of the arrival point (the full-galaxy import
must be re-run for the flag to exist); the coarse search then refuels on
the highway instead of detouring, which is where Spansh's Mandalay plot
gets its 14 fewer jumps. FSD injections are a last resort only when no plain
route exists (`RouteRequest::injection`).
