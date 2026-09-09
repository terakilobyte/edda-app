# EDDA project rules

## The measurement doctrine (maintainer-ruled, 2026-09-03 — binding on every agent and session)

**We do not make decisions blindly. Before we do anything, we measure.**

1. **Measure before building.** Any optimization, redesign, or "obvious
   improvement" starts with a measurement of the current behavior. If the
   baseline isn't measured, the first task is the instrument, not the
   feature. (Precedent: the binary-heap rewrite that one timer run
   cancelled; the CDC delta bench that killed two designs before a line
   of product code.)
2. **New things ship measurable.** Anything added must be observable the
   day it lands: tracing at phase boundaries with counts and durations,
   `edda_*` metrics series (naming conventions in
   `crates/ed-api/src/metrics.rs`), or a bench harness — whichever fits.
   "It works" without a number is not done.
3. **Benchmark constantly.** Performance claims come from benches, rerun
   when the code moves. Bench records are CSVs in `docs/benches/` with
   the verdict in the header; reusable harnesses live in
   `docs/benches/knobs/` as files, never retyped inline.
4. **Compare where it makes sense.** A/B against the current default with
   pinned inputs; pre-register the expected outcome before running.
   Pinned routes are tests — a change that worsens a pin fails.
5. **Keep the negatives.** Null and negative results go in the ledger
   in `docs/ROADMAP.md` under "Buried" (and in the bench CSV's verdict
   header) with their numbers, permanently. Buried ideas
   stay buried until a *premise* changes — then re-bench, don't
   re-litigate. (Precedent: ALT, buried on measurement, exhumed on a
   premise change, vindicated by re-measurement.)

When a claim and a measurement disagree, the measurement wins. When two
measurements disagree, find the broken instrument before trusting either.

## House conventions

- `docs/ROADMAP.md` is the project's record: open work, decisions with
  their dates, buried ideas with the numbers that buried them. Bench
  verdicts live in the CSV headers under `docs/benches/`; the reasoning
  behind a change goes in its pull request.
- Rust edits from an assistant session happen in a worktree
  (`.claude/worktrees/`) with a separate `CARGO_TARGET_DIR` — edits to
  the main tree restart the user's dev app.
- Tests must not speak: voice stays muted under test (`test_state`);
  audible tests only via `--features test-voice`.
- The player is addressed as "Commander" only — never sir/ma'am, in any
  persona, callout, or prompt.
- **The maintainer flies before we ship** (maintainer-ruled, 2026-09-06, after the
  vanishing-station spiral shipped in a release he had not flown): no
  release is cut until the maintainer has flown the change in the dev app.
  Tests gate the merge; the flight gates the release.
- **EDDA is not a surveillance tool** (maintainer-ruled, 2026-09-05). Nothing
  EDDA collects, aggregates, or displays may identify a commander or
  reveal where any individual is or has been. The heatmap is deliberately
  low-resolution and nameless; feedback is anonymous by wire contract;
  telemetry is a closed allowlist (warn/error callsites, timings,
  opt-in flags — never rendered messages, positions, or journal
  content). When a feature idea needs individual-level data to work,
  the feature changes shape or does not ship. **The canonical test
  (maintainer, 2026-09-06): one EDDA user can never figure out the location
  of another EDDA user in the app.** Fetching data from our API or
  from EDSM/Spansh/Inara, as commanders already do by hand, is not
  surveillance; letting one commander see where another is or was, by
  any path, is.
