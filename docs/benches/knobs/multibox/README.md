# Two-account journal folder instruments (2026-10-24)

Counts and shapes only: commanders are labelled A, B by first appearance;
no name, FID, system or balance is printed. Record:
`docs/benches/2026-10-24-two-account-folder-baseline.csv`.

- `survey.py <folder>` -- FIDs, files per commander, concurrency, menu-only files.
- `survey2.py <folder>` -- what each commander does, concurrency detail, relogs.
- `flips.py <store>` -- how often each "newest row wins" reader changes commander.
- `ambiguity.py <store>` -- timing attribution of companion files, by window.
- `balance.py <store>` -- whether Status.json's Balance can pick the commander.

`<store>` is the folder replayed through
`cargo run -p ed-store --release --example first_sync -- <folder> <scratch.sqlite3>`.
- `cargo run -p ed-store --release --example multibox_replay -- <folder> <scratch>` -- the
  folder through the registry: stores, purity, companion attribution, quiet-sync cost.
  Record: `docs/benches/2026-10-24-multibox-replay.csv`.
