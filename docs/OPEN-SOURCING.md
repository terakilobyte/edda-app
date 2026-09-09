# Open-sourcing scrub (2026-09-09)

Six read-only audits ran over the tree and the full git history (secrets,
history, personal data, licensing, hygiene, the ledger); their findings
were applied in one pass. What was done, and what is left for the
maintainer to decide.

## Done

- **History**: no credential, key, token, journal or personal game data
  was ever committed on any branch; pack is 18.5 MiB. No rewrite needed.
  `.mailmap` maps the maintainer's two addresses to one identity.
- **Secrets**: none in the tree. The production box's IPv4/IPv6 and
  `root@` target are gone from deploy docs, release scripts and the
  release workflow (`vars.DEPLOY_HOST` / `EDDA_DEPLOY_HOST`); the minisign
  signing key's on-disk path is gone from two scripts and the ledger
  (`EDDA_UPDATER_KEY` is required); personal machine paths are gone from
  seven bench scripts, three tools and four docs.
- **Personal data**: the maintainer's commander name replaced by the
  canonical fictional one in two tests; the ledger redacted in 47 places
  (exact session timestamps and logout positions, a real journal
  filename, ship names, a squadron carrier callsign, real-life routine
  mentions, five verbatim profane quotes paraphrased, two Spansh result
  ids); a glossary at the top of the ledger explains "the boss",
  "Statler" and "Waldorf".
- **Naming**: ~140 files had "the boss" → "the maintainer" and the two
  assistant-session personas neutralised in comments and docs; quoted
  profanity in code comments paraphrased. The ledger and the published
  blog keep their voice.
- **Licensing**: `LICENSE` (MIT), `THIRD-PARTY-NOTICES.md`, `deny.toml`;
  license fields in `frontend/package.json` and the bench tool; the
  FDevIDs licence claim corrected in code and docs. No copyleft in the
  dependency graph beyond MPL-2.0 file-scoped crates (unmodified).
- **Onboarding**: `README.md` rewritten for a newcomer; `CONTRIBUTING.md`,
  `SECURITY.md`, `CODE_OF_CONDUCT.md`, `frontend/README.md`; `AGENTS.md`
  dangling references fixed; session hand-offs and agent plans moved to
  `docs/archive/`; the blog draft removed.
- **Hygiene**: `.gitignore` covers `.claude/`, `target/`, logs, databases,
  env files, caches; ~60 MB of bench artefacts (Optuna store, eight HTML
  reports, flamegraph SVGs, the 10 MB voxel CSV) untracked and ignored.

## Maintainer decisions before the repository turns public

1. **Screenshots** (`screenshots/*.png`, `site/assets/route_planner.png`,
   `trade_finder.png`, `hud_*.png`): they show the maintainer's commander
   name, ship name, dock and Powerplay pledge. Keep, or re-capture with
   a fresh profile.
2. **Privacy page vs the install id**: `site/privacy/index.html` says
   there is no install id; the 0.3.0 client sends `X-EDDA-Install` (a
   random per-install key for the rate limiters). Reconcile the page
   before 0.3.0 ships. The same page still describes "Use local data",
   which B.4 removed.
3. **Fandom prose in `crates/ed-engineering/data/synthesis.json`**: the
   recipe numbers are game facts; the copied sentences are CC BY-SA 3.0.
   Strip the prose or accept the licence on that file.
4. **Wanderer's Toolbox text in `engineer_unlocks.json` /
   `material_sources.json`**: unlicensed community guide text. Ask for
   permission or rewrite the note fields.
5. **Piper voice licences**: verify each voice's MODEL_CARD
   (`en_US-lessac-high` is trained on the Blizzard 2013 corpus) and
   record them in `THIRD-PARTY-NOTICES.md`.
6. **Scratch branches**: `arch/*`, `statler-*` and `origin/api-only-server`
   are working branches; prune before publishing so the branch list is
   `main` + release tags.
7. **cargo-deny in CI**: `deny.toml` is in place; run it online once for
   the Windows and Linux dependency graphs (237 crates could not be
   read offline) and add the step to `ci.yml`.
8. **Root deploy user**: CI deploys as `root` over SSH; a dedicated
   deploy user is the usual hardening.
9. **The ledger itself**: publishable with the redactions above; it
   still records the maintainer's own flights and trades in aggregate,
   which the blog already discloses. Keep or move to a private repo.
