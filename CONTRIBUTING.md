# Contributing to EDDA

Thanks for helping. This page is short on purpose; the rules in it are
the ones that have bitten us.

## Build and test

- Rust stable, Node.js 20+, the Tauri 2 CLI and your OS's Tauri
  dependencies (Linux: [docs/BUILDING-LINUX.md](docs/BUILDING-LINUX.md)).
- App: `cd frontend && npm ci`, then `cargo tauri dev` from `src-tauri/`.
- Server: `docker compose up -d postgres`, copy `.env.example` to `.env`,
  `cargo build --release -p ed-api`; the rest is in
  [crates/ed-api/README.md](crates/ed-api/README.md).
- Tests: `cargo test --workspace` and `cd frontend && npx vitest run`.
  The server's PostgreSQL tests are `#[ignore]` and run with
  `EDDA_API_TEST_DATABASE_URL` set. `cargo clippy --workspace --all-targets`
  runs in CI too.

## Branches, pull requests, releases

`main` is protected: every change lands through a pull request, and CI
(Linux tests, clippy, frontend tests and build) must pass. Releases are
`vX.Y.Z` tags; the release workflow verifies the tag against
`src-tauri/Cargo.toml` and the newest section of
`src-tauri/RELEASE-NOTES.md`, builds and signs the installers, deploys the
server and publishes the GitHub Release. A user-visible change needs a
paragraph in the release notes.

## Measure first

EDDA does not make decisions blindly. Before we build, we measure.

1. **Measure before building.** An optimization, a redesign or an
   "obvious improvement" starts with a number for the current behavior. If
   there is no baseline, the first task is the instrument, not the feature.
2. **New things ship measurable.** Anything you add must be observable the
   day it lands: tracing at phase boundaries with counts and durations, an
   `edda_*` metrics series (conventions in `crates/ed-api/src/metrics.rs`),
   or a bench harness. "It works" without a number is not done.
3. **Benchmark, and keep the record.** Performance claims come from
   benches, rerun when the code moves. Bench records are CSVs in
   `docs/benches/` with the verdict in the header; reusable harnesses live
   in `docs/benches/knobs/` as files, never retyped inline.
4. **Compare fairly.** A/B against the current default with pinned inputs,
   and write down the expected outcome before you run. Pinned routes are
   tests: a change that worsens a pin fails.
5. **Keep the negatives.** Null and negative results go in the project
   ledger (kept privately by the maintainer; `docs/ROADMAP.md` is the public view), with their
   numbers, permanently. A buried idea stays buried until a premise
   changes; then re-bench rather than re-argue.

When a claim and a measurement disagree, the measurement wins. When two
measurements disagree, find the broken instrument before trusting either.

## The flight rule

Tests gate the merge; the flight gates the release. No release is cut
until a maintainer has flown the change in the app, in game. If your
change touches what a pilot sees or hears, say in the PR what to fly to
see it.

## Privacy is a design constraint

EDDA is not a surveillance tool. Nothing it collects, aggregates or
displays may identify a commander or reveal where any individual is or
has been; one EDDA user must never be able to work out where another is
or was. As a contribution rule:

- A feature that needs individual-level data to work changes shape or
  does not ship. Ask before building it.
- Never log commander names, positions, rendered messages or journal
  content, on the client or the server. Telemetry is a closed allowlist;
  extending it is a reviewed change, not a `warn!` with a payload.
- Feedback is anonymous by wire contract. Keep it that way.

## House conventions

- **The player is "Commander".** Never sir, ma'am or a name, in any
  persona, callout, prompt or UI string.
- **Tests must not speak.** Voice stays muted under test (`test_state`);
  a test that must be audible runs only under
  `cargo test --features test-voice`.
- **The ledger.** Decisions, bench verdicts and retractions go in
  the ledger as they happen, so the next person does not
  re-run a settled experiment.
- **Attribution.** Anything ported from another project gets an in-source
  header naming its origin and an entry in `docs/THIRD-PARTY.md`.
- **Release notes** are user-facing prose in `src-tauri/RELEASE-NOTES.md`,
  compiled into the app and published on the site. Lead with one paragraph
  a pilot would want to read.

## Reporting problems

Bugs and feature requests go to GitHub Issues. Security problems go
through [SECURITY.md](SECURITY.md), not a public issue. Conduct problems
follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
