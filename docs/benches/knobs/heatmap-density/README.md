# Heatmap tick-density measurement — SQL half RETIRED (2026-09-04)

The original `measure.sql` never ran, for two reasons kept here per
doctrine rule 5:

1. **Null result (review):** the sizing DB holds NO observation
   history — `market` is newer-wins current state (each EDDN board
   overwrites the last), `stations` keeps latest watermarks only.
   There are no observation tables to measure station-day tick depth
   from, retroactively or otherwise; that absence is exactly why the
   elasticity study needed a live poller (`board_watch.sh`).
2. **Premise change (maintainer ruling, ledger 1bdf669):** stored activity
   history is out by architecture — the heatmap is a pure additive
   function with universal decay. The SQL's verdict was to gate a
   historical build that no longer exists to gate.

The SURVIVING instrument is the app's own live counters: the
`activity_heatmap` snapshot's `seen` / `placed` / `unplaced` and the
Galaxy tab's `/min heard` readout. If a density number is ever wanted
again, the honest instrument is a `board_watch`-style sampler or a
server-side tick counter — do not build either for a buried
candidate.
