# Multiple Commanders Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One EDDA process keeps every Frontier account in its own SQLite store, shows the commander the player selects, and speaks only that commander, with danger-only alerts from the others behind an off-by-default toggle.

**Architecture:** A new `ed_store::registry::Registry` owns one `Store` per FID (`edda.<FID>.sqlite3` beside today's `edda.sqlite3`), attributes every journal file to its FID from its `Commander`/`LoadGame` line, routes each file's bytes to its store, and attributes the companion files (Status.json and friends) by content, never by timing. `AppState` holds the registry and a selected FID; `with_store`/`with_read` serve the selected store, so none of the ~140 existing query sites change. The watcher runs one pass per store; the unselected stores' callouts are filtered to the danger set.

**Tech Stack:** Rust (rusqlite, serde_json, tempfile in tests), Tauri 2 commands, Svelte 5 with vitest SSR render tests, Python knobs under `docs/benches/knobs/multibox/`.

**Spec:** `docs/superpowers/specs/2026-10-24-multi-commander-design.md`

## Global Constraints

- Rust edits happen in the worktree `C:\Users\terak\ED-Claude\edda-app\.claude\worktrees\planner-rim` on branch `multi-commander`, with `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits` on every cargo command. Never build or edit in the main tree (it restarts the maintainer's dev app).
- TDD: every task writes the failing test first and runs it RED before any production code.
- The donor's journal folder (`C:\Users\terak\Downloads\Elite Dangerous\Elite Dangerous`) stays where it is. Nothing from it enters the repo: no name, FID, system, balance or journal line. Harness output prints counts, labels and file names only.
- Per-FID store file name: `edda.<FID>.sqlite3`, a sibling of the base path, so `schema::galaxy_path(db_path)` (`galaxy.sqlite3` beside it) stays shared.
- Companion files are attributed by content (Status.json by Balance, Cargo.json by Count, Market/Outfitting/Shipyard by MarketID, NavRoute by first hop, ShipLocker/Backpack by item totals, ModulesInfo by module set). Unknown is the literal snapshot `{}`; readers already treat a missing field as none.
- A journal file belongs to one FID; a line naming a second FID is refused from that line with one `warn` log line, never blended.
- A single-commander install behaves exactly as today: one store, no strip, companions always attributed to the only store.
- Copy: the player is "Commander" only, never sir/ma'am; another account is named by its commander name; nothing on screen identifies another EDDA user.
- No commit or PR carries AI attribution lines.
- Telemetry carries no FID, no name and no commander count.
- Commit messages name the maintainer's ruling and the measurement, as the repo's history does.

## Review Focus

1. Both game instances write within the same second and the selected commander's panel must never show the other's ship, cargo or missions: Task 4's `two_fids_in_one_folder_land_in_two_stores` and Task 5's replay verdict (zero foreign FIDs per store).
2. A player relogs to the main menu and back inside one session (1,492 times in the donor data): the file keeps its FID and keeps ingesting: Task 1's `a_second_commander_event_with_the_same_fid_is_the_same_owner`.
3. The launcher is opened and closed without entering the game (164 files): the file is ignored, not held forever and not counted as a commander: Task 1's `a_file_that_shut_down_without_a_commander_is_menu_only`, Task 4's `menu_only_files_open_no_store`.
4. An existing single-account install upgrades: its `edda.sqlite3` is renamed once, bookmarks survive, the app opens with no strip: Task 4's `legacy_store_with_one_fid_is_renamed_in_place`.
5. The alt is under attack while the main is selected with alerts off: nothing is spoken; with alerts on, one prefixed line is: Task 7's `other_commander_keeps_only_danger` and `other_commander_lines_carry_the_name`.

---

## File structure

- `crates/ed-journal/src/journal_file.rs` (modify): `Owner`, `owner_of_lines`, `owner` — who a journal file belongs to. Pure parsing, no store.
- `crates/ed-store/src/ingest.rs` (modify): `ingest_file_for` with an expected FID; `ingest_files` over a name list; `ingest_all` unchanged in signature as the single-store wrapper; `write_snapshot` and `mark_unknown` split out of `ingest_companions`.
- `crates/ed-store/src/session.rs` (modify): `credits_estimate`.
- `crates/ed-store/src/companions.rs` (create): `Score`, `score(conn, name, value)`; the per-file content rules.
- `crates/ed-store/src/registry.rs` (create): `Registry`, `Attribution`, `RegistrySync`; legacy migration; owner scan; routed sync; companion dispatch; `last_writer`.
- `crates/ed-store/src/lib.rs` (modify): `Store.fid`, `Store::open_for`, `Store::ingest_owned`, `Store::finish_sync`; `pub mod companions; pub mod registry;`.
- `crates/ed-store/examples/multibox_replay.rs` (create): the harness.
- `docs/benches/2026-10-24-multibox-replay.csv` (create): the harness verdict.
- `src-tauri/src/state.rs` (modify): `AppState` over the registry; `selected`; readers pool keyed by FID; `AppConfig.selected_commander`, `AppConfig.other_commander_alerts`.
- `src-tauri/src/commands.rs` (modify): `list_commanders`, `select_commander`, `other_commander_alerts`, `other_commander_alerts_set`; `db_stats` via `state.db_path()`.
- `src-tauri/src/capi.rs` (modify): `refresh_slot(fid)`, `migrate_legacy_slot`.
- `src-tauri/src/callouts.rs` (modify): `is_for_other_commander`, `prefix_for_other`.
- `src-tauri/src/watcher.rs` (modify): one pass per store; danger filter for unselected stores.
- `src-tauri/src/lib.rs` (modify): registry construction, `initial_sync`/`greet` over the registry, handler registration.
- `frontend/src/lib/api.js` (modify): four command rows and exports.
- `frontend/src/lib/CommanderStrip.svelte` (create); `frontend/src/App.svelte`, `frontend/src/lib/VoiceSettings.svelte`, `frontend/src/lib/SystemSettings.svelte`, `frontend/src/lib/Onboarding.svelte` (modify).
- `frontend/src/test/commanderstrip-renders.test.js` (create).
- `docs/ROADMAP.md` (modify): the entry.

---

### Task 1: Who owns a journal file (ed-journal)

**Files:**
- Modify: `crates/ed-journal/src/journal_file.rs`
- Test: same file, `#[cfg(test)] mod owner_tests`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  pub enum Owner { Fid(String), Pending, MenuOnly }
  pub fn owner_of_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Owner
  pub fn owner(path: &std::path::Path) -> std::io::Result<Owner>
  ```
  Rules: the first `Commander` or `LoadGame` line with a non-empty string `FID` decides `Fid`. No such line and the last parsable line is `Shutdown` → `MenuOnly`. Otherwise `Pending` (the file may still grow).

- [ ] **Step 1: Write the failing tests**

Append to `crates/ed-journal/src/journal_file.rs`:

```rust
#[cfg(test)]
mod owner_tests {
    use super::*;

    const HEAD: &str = r#"{"timestamp":"2026-10-17T18:06:48Z","event":"Fileheader","part":1,"language":"English/UK","Odyssey":true,"gameversion":"4.4.2.0","build":"r318000/r0 "}"#;
    const FRIEND: &str = r#"{"timestamp":"2026-10-17T18:06:50Z","event":"Friends","Status":"Online","Name":"Someone"}"#;
    const CMDR_A: &str = r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#;
    const LOAD_A: &str = r#"{"timestamp":"2026-10-17T18:06:56Z","event":"LoadGame","FID":"F0000001","Commander":"Alpha","Ship":"Mandalay","ShipID":31,"Credits":1000}"#;
    const CMDR_B: &str = r#"{"timestamp":"2026-10-17T18:30:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#;
    const SHUTDOWN: &str = r#"{"timestamp":"2026-10-17T18:07:00Z","event":"Shutdown"}"#;

    #[test]
    fn the_first_commander_line_names_the_owner() {
        assert_eq!(owner_of_lines([HEAD, FRIEND, FRIEND, CMDR_A, LOAD_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn a_loadgame_line_names_the_owner_when_commander_is_missing() {
        assert_eq!(owner_of_lines([HEAD, LOAD_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn a_second_commander_event_with_the_same_fid_is_the_same_owner() {
        // A relog to the menu and back writes Commander again (1,492 times in
        // the donor data); the file stays one commander's.
        assert_eq!(owner_of_lines([HEAD, CMDR_A, LOAD_A, CMDR_A, LOAD_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn the_first_fid_wins_even_if_a_later_line_names_another() {
        // Never observed (0 of 1,396 files); the ingest refuses the later
        // lines (Task 2). The owner is still the first.
        assert_eq!(owner_of_lines([HEAD, CMDR_A, CMDR_B]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn a_file_that_shut_down_without_a_commander_is_menu_only() {
        assert_eq!(owner_of_lines([HEAD, FRIEND, SHUTDOWN]), Owner::MenuOnly);
    }

    #[test]
    fn a_file_without_a_commander_that_has_not_shut_down_is_pending() {
        assert_eq!(owner_of_lines([HEAD, FRIEND]), Owner::Pending);
        assert_eq!(owner_of_lines(Vec::<&str>::new()), Owner::Pending);
    }

    #[test]
    fn junk_lines_are_skipped() {
        assert_eq!(owner_of_lines([HEAD, "not json", "", "\u{feff}", CMDR_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn owner_reads_a_file_from_disk() {
        let dir = std::env::temp_dir().join(format!("edda-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Journal.2026-10-17T180648.01.log");
        std::fs::write(&p, format!("{HEAD}\n{FRIEND}\n{CMDR_A}\n{LOAD_A}\n")).unwrap();
        assert_eq!(owner(&p).unwrap(), Owner::Fid("F0000001".into()));
        std::fs::write(&p, format!("{HEAD}\n{SHUTDOWN}\n")).unwrap();
        assert_eq!(owner(&p).unwrap(), Owner::MenuOnly);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-journal owner_tests`
Expected: FAIL to compile with `cannot find type Owner` / `cannot find function owner_of_lines`.

- [ ] **Step 3: Implement**

Add to `crates/ed-journal/src/journal_file.rs` (above the existing tests module; `serde_json` is already a dependency of ed-journal):

```rust
/// Who a journal file belongs to. The game writes one file per launch and
/// names the account in its `Commander` line (and again in `LoadGame`),
/// within the first ~45 lines; `Friends` and `Music` can precede it. A
/// file never changes hands (0 of a two-account player's 1,396 files did,
/// 2026-10-24): the first FID is the owner for the file's life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// The account's Frontier ID, e.g. `F1234567`.
    Fid(String),
    /// No FID yet and the file has not ended: read it again later.
    Pending,
    /// Ended (`Shutdown`) without ever naming a commander: the launcher
    /// was opened and closed at the menu. Nothing of value inside.
    MenuOnly,
}

/// Decide the owner from a file's lines, in order. Unparsable lines are
/// skipped, as the ingest skips them.
pub fn owner_of_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Owner {
    let mut last_event: Option<String> = None;
    for line in lines {
        let text = line.trim().trim_start_matches('\u{feff}');
        if text.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        let Some(event) = v.get("event").and_then(|e| e.as_str()) else {
            continue;
        };
        if matches!(event, "Commander" | "LoadGame") {
            if let Some(fid) = v.get("FID").and_then(|f| f.as_str()).filter(|f| !f.is_empty()) {
                return Owner::Fid(fid.to_string());
            }
        }
        last_event = Some(event.to_string());
    }
    if last_event.as_deref() == Some("Shutdown") {
        Owner::MenuOnly
    } else {
        Owner::Pending
    }
}

/// `owner_of_lines` over a file on disk. Stops at the first FID, so an
/// active file costs its first few kilobytes; a menu-only file is small.
pub fn owner(path: &std::path::Path) -> std::io::Result<Owner> {
    use std::io::BufRead as _;
    let f = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(f);
    let mut last_event: Option<String> = None;
    for line in reader.split(b'\n') {
        let line = line?;
        let Ok(text) = std::str::from_utf8(&line) else { continue };
        match owner_of_lines([text]) {
            Owner::Fid(fid) => return Ok(Owner::Fid(fid)),
            Owner::MenuOnly => last_event = Some("Shutdown".into()),
            Owner::Pending => {
                let t = text.trim().trim_start_matches('\u{feff}');
                if !t.is_empty() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
                        if let Some(e) = v.get("event").and_then(|e| e.as_str()) {
                            last_event = Some(e.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(if last_event.as_deref() == Some("Shutdown") { Owner::MenuOnly } else { Owner::Pending })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-journal`
Expected: all ed-journal tests PASS, including the 8 new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/ed-journal/src/journal_file.rs
git commit -m "Multiple commanders: a journal file's owner is the FID its first Commander or LoadGame line names (ed-journal journal_file::owner) -- Pending until it appears, MenuOnly when the file shut down without one; the shape measured on a two-account player's 1,396 files 2026-10-24 (no file ever carried two FIDs, 164 menu-only, the Commander line within 45 lines); test RED first"
```

---

### Task 2: Ingest routed by owner, with refusal of a second FID (ed-store)

**Files:**
- Modify: `crates/ed-store/src/ingest.rs`
- Test: same file, existing `mod tests`

**Interfaces:**
- Consumes: nothing from Task 1 (the ingest only checks the FID it is told).
- Produces:
  ```rust
  pub fn ingest_file_for(conn: &Connection, dir: &Path, name: &str, expect_fid: Option<&str>) -> Result<u64>
  pub fn ingest_files(conn: &Connection, dir: &Path, names: &[String], expect_fid: Option<&str>, progress: impl FnMut(&str, usize, usize)) -> Result<IngestStats>   // events only, no companions
  pub fn write_snapshot(conn: &Connection, name: &str, ts: Option<&str>, mtime: Option<i64>, raw: &str) -> Result<()>
  pub fn mark_unknown(conn: &Connection, name: &str) -> Result<()>   // raw = "{}", ts NULL, mtime NULL
  pub const UNKNOWN_SNAPSHOT: &str = "{}";
  ```
  `ingest_file(conn, dir, name)` becomes `ingest_file_for(conn, dir, name, None)`. `ingest_all` keeps its signature: `ingest_files` over `journal_file_names(dir)` with no expected FID, then `ingest_companions`.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/ed-store/src/ingest.rs`:

```rust
    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    #[test]
    fn a_line_naming_a_second_fid_is_refused_from_that_line_on() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Journal.2026-10-17T180648.01.log";
        std::fs::write(dir.path().join(name), concat!(
            r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:06:56Z","event":"LoadGame","FID":"F0000001","Commander":"Alpha","Ship":"Mandalay","ShipID":31}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:30:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:30:01Z","event":"LoadGame","FID":"F0000002","Commander":"Beta","Ship":"Type9","ShipID":4}"#, "\n",
        )).unwrap();
        let conn = mem();
        let n = ingest_file_for(&conn, dir.path(), name, Some("F0000001")).unwrap();
        assert_eq!(n, 2, "the two lines before the foreign FID land");
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM events WHERE json_extract(raw,'$.FID') = 'F0000002'"), 0);
        // The checkpoint moved past the refused lines: a second pass reads nothing.
        assert_eq!(ingest_file_for(&conn, dir.path(), name, Some("F0000001")).unwrap(), 0);
    }

    #[test]
    fn without_an_expected_fid_every_line_lands_as_today() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Journal.2026-10-17T180648.01.log";
        std::fs::write(dir.path().join(name), concat!(
            r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:30:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#, "\n",
        )).unwrap();
        let conn = mem();
        assert_eq!(ingest_file(&conn, dir.path(), name).unwrap(), 2);
    }

    #[test]
    fn ingest_files_reads_only_the_names_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let a = "Journal.2026-10-17T180648.01.log";
        let b = "Journal.2026-10-17T181000.01.log";
        std::fs::write(dir.path().join(a), concat!(r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#, "\n")).unwrap();
        std::fs::write(dir.path().join(b), concat!(r#"{"timestamp":"2026-10-17T18:10:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#, "\n")).unwrap();
        let conn = mem();
        let stats = ingest_files(&conn, dir.path(), &[a.to_string()], Some("F0000001"), |_, _, _| {}).unwrap();
        assert_eq!((stats.files_scanned, stats.files_changed, stats.events_inserted, stats.snapshots_updated), (1, 1, 1, 0));
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM events WHERE file = 'Journal.2026-10-17T181000.01.log'"), 0);
    }

    #[test]
    fn a_snapshot_can_be_written_and_marked_unknown() {
        let conn = mem();
        write_snapshot(&conn, "Status.json", Some("2026-10-17T18:06:55Z"), Some(1), r#"{"Fuel":{"FuelMain":12.5}}"#).unwrap();
        let fuel: Option<f64> = conn.query_row("SELECT json_extract(raw,'$.Fuel.FuelMain') FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(fuel, Some(12.5));
        mark_unknown(&conn, "Status.json").unwrap();
        let fuel: Option<f64> = conn.query_row("SELECT json_extract(raw,'$.Fuel.FuelMain') FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(fuel, None, "an unknown snapshot reads as no value, never as the other commander's");
        let raw: String = conn.query_row("SELECT raw FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(raw, UNKNOWN_SNAPSHOT);
        // Marking a name that was never written is fine and leaves a row.
        mark_unknown(&conn, "Cargo.json").unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM snapshots WHERE name='Cargo.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-store ingest::tests`
Expected: FAIL to compile: `cannot find function ingest_file_for`, `ingest_files`, `write_snapshot`, `mark_unknown`, `UNKNOWN_SNAPSHOT`.

- [ ] **Step 3: Implement**

In `crates/ed-store/src/ingest.rs`:

(a) Rename the existing `pub fn ingest_file(conn, dir, name)` to `pub fn ingest_file_for(conn: &Connection, dir: &Path, name: &str, expect_fid: Option<&str>) -> Result<u64>` and add the wrapper:

```rust
/// Tail one journal file into the store, every line (the single-store path).
pub fn ingest_file(conn: &Connection, dir: &Path, name: &str) -> Result<u64> {
    ingest_file_for(conn, dir, name, None)
}
```

(b) Inside `ingest_file_for`'s line loop, right after `let Some(event) = v.get("event")...` and before the `ts` binding, add the refusal:

```rust
            // A file belongs to one commander. A later line naming another
            // FID has never been seen (0 of 1,396 files, 2026-10-24); if a
            // game build ever does it, the rest of the file is refused with
            // one warning, never blended into this store.
            if let Some(expect) = expect_fid {
                if matches!(event, "Commander" | "LoadGame") {
                    if let Some(fid) = v.get("FID").and_then(|f| f.as_str()) {
                        if fid != expect {
                            tracing::warn!(file = name, offset = abs, found = fid, expected = expect, "journal line names another commander; the rest of this file is refused");
                            break;
                        }
                    }
                }
            }
```

(c) Add, after `ingest_file`:

```rust
/// Ingest the given journal files only (no companions). The registry hands
/// each store the names it owns; `expect_fid` is that store's commander.
pub fn ingest_files(
    conn: &Connection,
    dir: &Path,
    names: &[String],
    expect_fid: Option<&str>,
    mut progress: impl FnMut(&str, usize, usize),
) -> Result<IngestStats> {
    let total = names.len();
    let mut stats = IngestStats { files_scanned: total, ..Default::default() };
    for (i, name) in names.iter().enumerate() {
        progress(name, i + 1, total);
        let n = ingest_file_for(conn, dir, name, expect_fid)?;
        if n > 0 {
            stats.files_changed += 1;
            stats.events_inserted += n;
        }
    }
    Ok(stats)
}

/// The snapshot a store holds when a companion file could not be
/// attributed to it: every `json_extract` on it is NULL, so the fuel
/// gauge, flags and nav target show nothing rather than another
/// commander's values.
pub const UNKNOWN_SNAPSHOT: &str = "{}";

/// Upsert one companion snapshot.
pub fn write_snapshot(conn: &Connection, name: &str, ts: Option<&str>, mtime: Option<i64>, raw: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO snapshots (name, ts, mtime, raw) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET ts = excluded.ts, mtime = excluded.mtime, raw = excluded.raw",
        params![name, ts, mtime, raw],
    )?;
    Ok(())
}

/// This store does not know whose `name` is right now.
pub fn mark_unknown(conn: &Connection, name: &str) -> Result<()> {
    write_snapshot(conn, name, None, None, UNKNOWN_SNAPSHOT)
}
```

(d) Rewrite `ingest_all`'s body to use `ingest_files`:

```rust
    let names = journal_file_names(dir)?;
    let mut stats = ingest_files(conn, dir, &names, None, progress)?;
    stats.snapshots_updated = ingest_companions(conn, dir)?;
    Ok(stats)
```

(e) In `ingest_companions`, replace the inline `tx.execute("INSERT INTO snapshots ...")` with `write_snapshot(&tx, name, ts, mtime, &raw)?;` (the `Transaction` derefs to `Connection`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-store`
Expected: all ed-store tests PASS, the 4 new ones included; the existing `truncated_journal_file_archives_history_instead_of_deleting_it` still passes.

- [ ] **Step 5: Commit**

```bash
git add crates/ed-store/src/ingest.rs
git commit -m "Multiple commanders: ingest routed by owner -- ingest_files over a name list with the store's expected FID, a line naming another FID refused from that line on with one warning (never blended), write_snapshot and mark_unknown ('{}', so json_extract reads NULL) split out of the companion pass; ingest_all and ingest_file keep their signatures for the single store; tests RED first, ed-store green"
```

---

### Task 3: Companion files attributed by content (ed-store)

**Files:**
- Modify: `crates/ed-store/src/session.rs` (add `credits_estimate`)
- Create: `crates/ed-store/src/companions.rs`
- Modify: `crates/ed-store/src/lib.rs` (`pub mod companions;`)
- Test: `companions.rs` `mod tests`

**Interfaces:**
- Consumes: `ingest::write_snapshot`, `ingest::mark_unknown` (Task 2) are not called here; this task only scores.
- Produces:
  ```rust
  // session.rs
  pub fn credits_estimate(conn: &Connection) -> Result<Option<i64>>
  // companions.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Score { Exact, Distance(i64), Mismatch, Unknown }
  pub fn score(conn: &Connection, name: &str, v: &serde_json::Value) -> Result<Score>
  pub fn choose(scores: &[(String, Score)]) -> Vec<String>   // the FIDs that get the file; empty = unknown
  ```
  `choose`: one candidate → that one, whatever its score (a single-commander install is today's app). Otherwise: every `Exact` wins; if none, the smallest `Distance` wins when the runner-up distance is at least twice it (or there is no runner-up); `Mismatch` and `Unknown` never win.

- [ ] **Step 1: Write the failing tests**

Create `crates/ed-store/src/companions.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use serde_json::json;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }
    fn ev(conn: &Connection, ts: &str, offset: i64, v: serde_json::Value) {
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, system_address, market_id, raw) VALUES ('J.log', ?1, ?2, ?3, ?4, ?5, ?6)",
            params![offset, ts, v["event"].as_str().unwrap(), v.get("SystemAddress").and_then(|x| x.as_i64()), v.get("MarketID").and_then(|x| x.as_i64()), v.to_string()],
        ).unwrap();
    }

    #[test]
    fn credits_estimate_is_the_last_loadgame_plus_the_money_since() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"LoadGame","FID":"F1","Credits":1_000_000}));
        ev(&conn, "2026-10-17T18:05:00Z", 1, json!({"event":"MissionCompleted","Reward":250_000}));
        ev(&conn, "2026-10-17T18:06:00Z", 2, json!({"event":"MarketBuy","TotalCost":100_000}));
        ev(&conn, "2026-10-17T18:07:00Z", 3, json!({"event":"Bounty","TotalReward":999_999})); // a voucher, not paid yet
        ev(&conn, "2026-10-17T18:08:00Z", 4, json!({"event":"RedeemVoucher","Amount":50_000}));
        assert_eq!(crate::session::credits_estimate(&conn).unwrap(), Some(1_200_000));
        let empty = mem();
        assert_eq!(crate::session::credits_estimate(&empty).unwrap(), None);
    }

    #[test]
    fn status_scores_by_balance_distance() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"LoadGame","FID":"F1","Credits":5_000_000}));
        assert_eq!(score(&conn, "Status.json", &json!({"Balance": 5_010_000})).unwrap(), Score::Distance(10_000));
        assert_eq!(score(&conn, "Status.json", &json!({"Flags": 0})).unwrap(), Score::Unknown, "no Balance (on foot menus) is unknown");
        assert_eq!(score(&mem(), "Status.json", &json!({"Balance": 1})).unwrap(), Score::Unknown, "no LoadGame yet is unknown");
    }

    #[test]
    fn cargo_scores_by_count_and_vessel() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"Cargo","Vessel":"Ship","Count":64}));
        assert_eq!(score(&conn, "Cargo.json", &json!({"Vessel":"Ship","Count":64,"Inventory":[]})).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "Cargo.json", &json!({"Vessel":"Ship","Count":0,"Inventory":[]})).unwrap(), Score::Mismatch);
        assert_eq!(score(&mem(), "Cargo.json", &json!({"Vessel":"Ship","Count":0})).unwrap(), Score::Unknown);
    }

    #[test]
    fn market_outfitting_shipyard_score_by_market_id() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"Docked","MarketID":3_700_000_001i64,"StationName":"X"}));
        for name in ["Market.json", "Outfitting.json", "Shipyard.json"] {
            assert_eq!(score(&conn, name, &json!({"MarketID": 3_700_000_001i64})).unwrap(), Score::Exact, "{name}");
            assert_eq!(score(&conn, name, &json!({"MarketID": 3_700_000_002i64})).unwrap(), Score::Mismatch, "{name}");
        }
        assert_eq!(score(&mem(), "Market.json", &json!({"MarketID": 1})).unwrap(), Score::Unknown);
    }

    #[test]
    fn navroute_scores_by_its_first_hop_against_the_current_system() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"FSDJump","StarSystem":"A","SystemAddress":11}));
        let here = json!({"Route":[{"StarSystem":"A","SystemAddress":11},{"StarSystem":"B","SystemAddress":12}]});
        let elsewhere = json!({"Route":[{"StarSystem":"C","SystemAddress":13}]});
        assert_eq!(score(&conn, "NavRoute.json", &here).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "NavRoute.json", &elsewhere).unwrap(), Score::Mismatch);
        assert_eq!(score(&conn, "NavRoute.json", &json!({"Route":[]})).unwrap(), Score::Unknown, "a cleared route names nobody");
    }

    #[test]
    fn shiplocker_and_backpack_score_by_item_totals() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"ShipLocker","Items":[{"Name":"a","Count":2}],"Components":[{"Name":"b","Count":3}],"Consumables":[],"Data":[{"Name":"c","Count":1}]}));
        ev(&conn, "2026-10-17T18:01:00Z", 1, json!({"event":"ShipLocker"})); // the empty pointer the game writes later
        assert_eq!(score(&conn, "ShipLocker.json", &json!({"Items":[{"Name":"a","Count":2}],"Components":[{"Name":"b","Count":3}],"Consumables":[],"Data":[{"Name":"c","Count":1}]})).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "ShipLocker.json", &json!({"Items":[],"Components":[],"Consumables":[],"Data":[]})).unwrap(), Score::Mismatch);
        assert_eq!(score(&mem(), "Backpack.json", &json!({"Items":[]})).unwrap(), Score::Unknown);
    }

    #[test]
    fn modulesinfo_scores_by_the_module_set_of_the_latest_loadout() {
        let conn = mem();
        ev(&conn, "2026-10-17T18:00:00Z", 0, json!({"event":"Loadout","Ship":"mandalay","ShipID":31,"Modules":[{"Slot":"PowerPlant","Item":"int_powerplant_size5_class5"},{"Slot":"FrameShiftDrive","Item":"int_hyperdrive_overcharge_size5_class5"}]}));
        assert_eq!(score(&conn, "ModulesInfo.json", &json!({"Modules":[{"Slot":"FrameShiftDrive","Item":"int_hyperdrive_overcharge_size5_class5"},{"Slot":"PowerPlant","Item":"int_powerplant_size5_class5"}]})).unwrap(), Score::Exact);
        assert_eq!(score(&conn, "ModulesInfo.json", &json!({"Modules":[{"Slot":"PowerPlant","Item":"int_powerplant_size3_class1"}]})).unwrap(), Score::Mismatch);
    }

    #[test]
    fn an_unlisted_companion_is_unknown() {
        assert_eq!(score(&mem(), "FCMaterials.json", &json!({})).unwrap(), Score::Unknown);
    }

    #[test]
    fn choose_gives_a_single_store_everything() {
        assert_eq!(choose(&[("F1".to_string(), Score::Unknown)]), vec!["F1".to_string()]);
        assert_eq!(choose(&[("F1".to_string(), Score::Mismatch)]), vec!["F1".to_string()]);
    }

    #[test]
    fn choose_prefers_exact_then_a_clear_nearest_balance() {
        let f = |s: &str| s.to_string();
        assert_eq!(choose(&[(f("F1"), Score::Exact), (f("F2"), Score::Mismatch)]), vec![f("F1")]);
        assert_eq!(choose(&[(f("F1"), Score::Exact), (f("F2"), Score::Exact)]), vec![f("F1"), f("F2")], "a tie with equal content goes to both");
        assert_eq!(choose(&[(f("F1"), Score::Distance(1_000)), (f("F2"), Score::Distance(2_000_000_000))]), vec![f("F1")]);
        assert_eq!(choose(&[(f("F1"), Score::Distance(1_000)), (f("F2"), Score::Distance(1_500))]), Vec::<String>::new(), "too close to call is unknown");
        assert_eq!(choose(&[(f("F1"), Score::Distance(1_000)), (f("F2"), Score::Unknown)]), vec![f("F1")]);
        assert_eq!(choose(&[(f("F1"), Score::Mismatch), (f("F2"), Score::Unknown)]), Vec::<String>::new());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-store companions`
Expected: FAIL to compile (`mod companions` is not declared; add `pub mod companions;` to `crates/ed-store/src/lib.rs` beside the other `pub mod` lines first, then the failure is `cannot find function score` / `choose` / `credits_estimate`).

- [ ] **Step 3: Implement**

Add to `crates/ed-store/src/session.rs`:

```rust
/// The commander's credits as the journal lets us estimate them: the last
/// `LoadGame`'s `Credits` plus every paid-out or paid-in amount since.
/// Vouchers (`Bounty`, `FactionKillBond`) are not money until redeemed.
/// Used to tell whose Status.json a Balance belongs to when two accounts
/// share one folder (their balances were never close on the donor data;
/// figures withheld, balance-derived).
pub fn credits_estimate(conn: &Connection) -> Result<Option<i64>> {
    let Some((ts, file, offset, credits)) = conn
        .query_row(
            "SELECT ts, file, offset, json_extract(raw, '$.Credits') FROM events
             WHERE event = 'LoadGame' AND json_extract(raw, '$.Credits') IS NOT NULL
             ORDER BY ts DESC, file DESC, offset DESC LIMIT 1",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    // (event, json field, sign)
    const MONEY: &[(&str, &str, i64)] = &[
        ("MissionCompleted", "$.Reward", 1),
        ("RedeemVoucher", "$.Amount", 1),
        ("MarketSell", "$.TotalSale", 1),
        ("MarketBuy", "$.TotalCost", -1),
        ("RefuelAll", "$.Cost", -1),
        ("BuyAmmo", "$.Cost", -1),
        ("RepairAll", "$.Cost", -1),
        ("Repair", "$.Cost", -1),
        ("ShipyardBuy", "$.ShipPrice", -1),
        ("ShipyardSell", "$.ShipPrice", 1),
        ("ModuleBuy", "$.BuyPrice", -1),
        ("ModuleSell", "$.SellPrice", 1),
        ("PayFines", "$.Amount", -1),
        ("PayBounties", "$.Amount", -1),
        ("BuyDrones", "$.TotalCost", -1),
        ("SellDrones", "$.TotalSale", 1),
        ("SellExplorationData", "$.TotalEarnings", 1),
        ("MultiSellExplorationData", "$.TotalEarnings", 1),
        ("Resurrect", "$.Cost", -1),
        ("CarrierDepositFuel", "$.Total", -1),
        ("CarrierBankTransfer", "$.Deposit", -1),
        ("CarrierBankTransfer", "$.Withdraw", 1),
        ("CommunityGoalReward", "$.Reward", 1),
        ("PowerplaySalary", "$.Amount", 1),
    ];
    let mut total = credits;
    let mut stmt = conn.prepare_cached(
        "SELECT COALESCE(SUM(json_extract(raw, ?1)), 0) FROM events
         WHERE event = ?2 AND (ts > ?3 OR (ts = ?3 AND (file > ?4 OR (file = ?4 AND offset > ?5))))",
    )?;
    for (event, field, sign) in MONEY {
        let sum: i64 = stmt.query_row(rusqlite::params![field, event, ts, file, offset], |r| r.get::<_, f64>(0).map(|f| f as i64))?;
        total += sign * sum;
    }
    Ok(Some(total))
}
```

(`OptionalExtension` is already imported in session.rs; if not, add `use rusqlite::OptionalExtension as _;`.)

Fill `crates/ed-store/src/companions.rs` above the tests:

```rust
//! Whose companion file is this? The game keeps one Status.json, one
//! Cargo.json and so on per Windows user, with no FID inside, and two
//! instances overwrite the same files. Timing cannot tell them apart
//! (15.6% of state lines ambiguous at 3 s on the donor data,
//! `docs/benches/2026-10-24-two-account-folder-baseline.csv`); content
//! can. Each rule compares the file with what the store's own journal
//! last said.
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension as _};
use serde_json::Value;

/// How well a companion file fits one commander's store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Score {
    /// The file says what this store's journal says.
    Exact,
    /// Status.json only: how far its Balance is from this store's credits.
    Distance(i64),
    /// The file contradicts this store's journal.
    Mismatch,
    /// This store cannot say (no journal fact to compare, or the file
    /// carries nothing to compare).
    Unknown,
}

fn latest_raw(conn: &Connection, event: &str, having: Option<&str>) -> Result<Option<Value>> {
    let sql = match having {
        Some(field) => format!("SELECT raw FROM events WHERE event = ?1 AND json_extract(raw, '{field}') IS NOT NULL ORDER BY ts DESC, file DESC, offset DESC LIMIT 1"),
        None => "SELECT raw FROM events WHERE event = ?1 ORDER BY ts DESC, file DESC, offset DESC LIMIT 1".to_string(),
    };
    let raw: Option<String> = conn.query_row(&sql, [event], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

fn item_total(v: &Value) -> Option<i64> {
    let mut total = 0;
    let mut any = false;
    for key in ["Items", "Components", "Consumables", "Data"] {
        if let Some(arr) = v.get(key).and_then(Value::as_array) {
            any = true;
            total += arr.iter().filter_map(|i| i.get("Count").and_then(Value::as_i64)).sum::<i64>();
        }
    }
    any.then_some(total)
}

fn module_set(v: &Value) -> Option<std::collections::BTreeSet<(String, String)>> {
    let arr = v.get("Modules")?.as_array()?;
    Some(arr.iter().filter_map(|m| Some((m.get("Slot")?.as_str()?.to_ascii_lowercase(), m.get("Item")?.as_str()?.to_ascii_lowercase()))).collect())
}

fn eq_or_mismatch(same: bool) -> Score {
    if same { Score::Exact } else { Score::Mismatch }
}

/// Score `v` (the parsed companion file `name`) against this store.
pub fn score(conn: &Connection, name: &str, v: &Value) -> Result<Score> {
    Ok(match name {
        "Status.json" => {
            let (Some(balance), Some(est)) = (v.get("Balance").and_then(Value::as_i64), crate::session::credits_estimate(conn)?) else {
                return Ok(Score::Unknown);
            };
            Score::Distance((balance - est).abs())
        }
        "Cargo.json" => {
            let Some(last) = latest_raw(conn, "Cargo", Some("$.Count"))? else { return Ok(Score::Unknown) };
            let Some(count) = v.get("Count").and_then(Value::as_i64) else { return Ok(Score::Unknown) };
            let vessel = v.get("Vessel").and_then(Value::as_str).unwrap_or("Ship");
            eq_or_mismatch(last.get("Count").and_then(Value::as_i64) == Some(count) && last.get("Vessel").and_then(Value::as_str).unwrap_or("Ship") == vessel)
        }
        "Market.json" | "Outfitting.json" | "Shipyard.json" => {
            let Some(id) = v.get("MarketID").and_then(Value::as_i64) else { return Ok(Score::Unknown) };
            let docked: Option<i64> = conn
                .query_row("SELECT market_id FROM events WHERE event = 'Docked' AND market_id IS NOT NULL ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
                .optional()?;
            match docked {
                None => Score::Unknown,
                Some(d) => eq_or_mismatch(d == id),
            }
        }
        "NavRoute.json" => {
            let Some(first) = v.get("Route").and_then(Value::as_array).and_then(|r| r.first()) else { return Ok(Score::Unknown) };
            let Some(start) = first.get("SystemAddress").and_then(Value::as_i64) else { return Ok(Score::Unknown) };
            let here: Option<i64> = conn
                .query_row("SELECT system_address FROM events WHERE event IN ('FSDJump','Location','CarrierJump') AND system_address IS NOT NULL ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
                .optional()?;
            match here {
                None => Score::Unknown,
                Some(h) => eq_or_mismatch(h == start),
            }
        }
        "ShipLocker.json" | "Backpack.json" => {
            let event = name.trim_end_matches(".json");
            let Some(last) = latest_raw(conn, event, Some("$.Items"))? else { return Ok(Score::Unknown) };
            match (item_total(v), item_total(&last)) {
                (Some(a), Some(b)) => eq_or_mismatch(a == b),
                _ => Score::Unknown,
            }
        }
        "ModulesInfo.json" => {
            let Some(last) = latest_raw(conn, "Loadout", Some("$.Modules"))? else { return Ok(Score::Unknown) };
            match (module_set(v), module_set(&last)) {
                (Some(a), Some(b)) => eq_or_mismatch(a == b),
                _ => Score::Unknown,
            }
        }
        _ => Score::Unknown,
    })
}

/// Which stores receive the file. One store gets everything (a single
/// commander is today's app). Otherwise every `Exact` store does; failing
/// that, the clearly nearest Balance (the runner-up at least twice as far).
/// Nobody means unknown: no store takes it and each is marked so.
pub fn choose(scores: &[(String, Score)]) -> Vec<String> {
    if scores.len() == 1 {
        return vec![scores[0].0.clone()];
    }
    let exact: Vec<String> = scores.iter().filter(|(_, s)| *s == Score::Exact).map(|(f, _)| f.clone()).collect();
    if !exact.is_empty() {
        return exact;
    }
    let mut dists: Vec<(i64, &String)> = scores.iter().filter_map(|(f, s)| if let Score::Distance(d) = s { Some((*d, f)) } else { None }).collect();
    dists.sort();
    match dists.as_slice() {
        [] => Vec::new(),
        [(_, only)] => vec![(*only).clone()],
        [(best, f), (next, _), ..] if *next >= best.saturating_mul(2) => vec![(*f).clone()],
        _ => Vec::new(),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-store`
Expected: PASS, including the 11 new tests. If `credits_estimate_is_the_last_loadgame_plus_the_money_since` fails on the sum type, SQLite returns `SUM` of JSON integers as INTEGER: change the read to `r.get::<_, i64>(0)` and keep the test.

- [ ] **Step 5: Commit**

```bash
git add crates/ed-store/src/companions.rs crates/ed-store/src/session.rs crates/ed-store/src/lib.rs
git commit -m "Multiple commanders: companion files attributed by content, never by timing -- Status.json by Balance against the store's credits estimate (last LoadGame plus the money events since; vouchers are not money), Cargo by Count, Market/Outfitting/Shipyard by MarketID, NavRoute by its first hop against the current system, ShipLocker/Backpack by item totals, ModulesInfo by the module set; choose(): one store takes all, Exact wins, else the clearly nearest balance (runner-up twice as far), else unknown -- measured 2026-10-24: timing was ambiguous for 15.6% of state lines at 3 s, the two balances never within 5.3 M Cr; tests RED first"
```

---

### Task 4: The registry: one store per commander (ed-store)

**Files:**
- Modify: `crates/ed-store/src/lib.rs` (`Store.fid`, `Store::open_for`, `Store::ingest_owned`, `Store::finish_sync`, `pub mod registry;`)
- Create: `crates/ed-store/src/registry.rs`
- Test: `registry.rs` `mod tests`

**Interfaces:**
- Consumes: `ed_journal::journal_file::{owner, Owner}` (Task 1); `ingest::{journal_file_names, ingest_files, write_snapshot, mark_unknown, COMPANION_FILES, IngestStats}` (Task 2); `companions::{score, choose}` (Task 3).
- Produces:
  ```rust
  // lib.rs
  impl Store {
      pub fn open_for(db_path: &Path, journal_dir: &Path, fid: Option<&str>) -> Result<Self>   // open() = open_for(.., None)
      pub fn fid(&self) -> Option<&str>
      pub fn ingest_owned(&self, names: &[String], progress: impl FnMut(&str, usize, usize)) -> Result<IngestStats>  // ingest_files with self.fid, no companions, no derive
      pub fn finish_sync(&self, ingest: IngestStats, started: std::time::Instant) -> Result<SyncStats>   // derive + merit capture; sync_with_progress is now ingest_all + finish_sync
  }
  // registry.rs
  pub struct Registry { .. }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum Attribution { To(Vec<String>), Unknown }
  #[derive(Debug, Default)]
  pub struct RegistrySync {
      pub per_store: std::collections::BTreeMap<String, SyncStats>,
      pub new_fids: Vec<String>,
      pub companions: Vec<(String, Attribution)>,
      pub menu_only_files: usize,
      pub pending_files: usize,
      pub elapsed_ms: u64,
  }
  pub const PLACEHOLDER: &str = "";   // the key of the FID-less store a fresh install starts with
  impl Registry {
      pub fn open(base: &Path, journal_dir: &Path) -> Result<Registry>
      pub fn db_path_for(base: &Path, fid: &str) -> PathBuf   // "" → base itself; else base.with_file_name(format!("edda.{fid}.sqlite3"))
      pub fn journal_dir(&self) -> &Path
      pub fn base(&self) -> &Path
      pub fn fids(&self) -> Vec<String>              // real FIDs only, sorted; never PLACEHOLDER
      pub fn keys(&self) -> Vec<String>              // fids(), or [PLACEHOLDER] while there are none
      pub fn store(&self, key: &str) -> Option<Arc<Mutex<Store>>>
      pub fn stores(&self) -> Vec<(String, Arc<Mutex<Store>>)>
      pub fn last_writer(&self) -> Option<String>    // the FID owning the journal file with the newest mtime
      pub fn sync_all(&self, progress: impl FnMut(&str, usize, usize)) -> Result<RegistrySync>
  }
  ```
  Legacy migration inside `open`: `base` exists and no `edda.*.sqlite3` sibling exists →
  - one distinct FID among its `Commander` events: `PRAGMA wal_checkpoint(TRUNCATE)`, close, rename `base` to `db_path_for(base, fid)`, delete `base-wal`/`base-shm` if present;
  - no FID: keep `base` as the placeholder store;
  - two or more: for each FID open a fresh store, `ATTACH` the legacy file as `legacy` and `INSERT INTO user_marks SELECT * FROM legacy.user_marks`, detach; rename the legacy file to `edda.blended.sqlite3`; `warn!` once. The journal re-ingests on the first sync.
  `sync_all`: list `journal_file_names`; for each name whose owner is unknown or `Pending`, call `owner(path)`; a new `Fid` opens its store (`new_fids`) and, if the placeholder store exists and has zero events, that placeholder is dropped from the map (its file is left on disk). Then for each FID store: `ingest_owned(owned names)`. Then each companion file: read, parse (skip half-written), `score` against every FID store, `choose`; `write_snapshot` into the chosen, `mark_unknown` into the others only when nobody was chosen (`Attribution::Unknown`). Then each store `finish_sync`. The placeholder store (no FIDs yet) runs today's `sync_with_progress` so a fresh install behaves as it does today.

- [ ] **Step 1: Write the failing tests**

Create `crates/ed-store/src/registry.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CMDR_A: &str = r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#;
    const LOAD_A: &str = r#"{"timestamp":"2026-10-17T18:06:56Z","event":"LoadGame","FID":"F0000001","Commander":"Alpha","Ship":"Mandalay","ShipID":31,"Credits":5000000}"#;
    const CMDR_B: &str = r#"{"timestamp":"2026-10-17T18:07:55Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#;
    const LOAD_B: &str = r#"{"timestamp":"2026-10-17T18:07:56Z","event":"LoadGame","FID":"F0000002","Commander":"Beta","Ship":"Type9","ShipID":4,"Credits":9000000000}"#;
    const CARGO_A: &str = r#"{"timestamp":"2026-10-17T18:08:00Z","event":"Cargo","Vessel":"Ship","Count":0}"#;
    const CARGO_B: &str = r#"{"timestamp":"2026-10-17T18:08:01Z","event":"Cargo","Vessel":"Ship","Count":700}"#;
    const MENU: &str = concat!(r#"{"timestamp":"2026-10-17T19:00:00Z","event":"Fileheader","part":1}"#, "\n", r#"{"timestamp":"2026-10-17T19:00:05Z","event":"Shutdown"}"#, "\n");

    fn folder() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&journal).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        (dir, journal, data.join("edda.sqlite3"))
    }
    fn count(store: &Arc<Mutex<Store>>, sql: &str) -> i64 {
        store.lock().unwrap().conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn two_fids_in_one_folder_land_in_two_stores() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n{CARGO_A}\n")).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}\n{LOAD_B}\n{CARGO_B}\n")).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        let out = reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(reg.fids(), vec!["F0000001".to_string(), "F0000002".to_string()]);
        assert_eq!(out.new_fids, vec!["F0000001".to_string(), "F0000002".to_string()]);
        let a = reg.store("F0000001").unwrap();
        let b = reg.store("F0000002").unwrap();
        assert_eq!(count(&a, "SELECT COUNT(*) FROM events"), 3);
        assert_eq!(count(&b, "SELECT COUNT(*) FROM events"), 3);
        assert_eq!(count(&a, "SELECT COUNT(*) FROM events WHERE json_extract(raw,'$.FID') = 'F0000002'"), 0);
        assert_eq!(count(&b, "SELECT COUNT(*) FROM events WHERE json_extract(raw,'$.FID') = 'F0000001'"), 0);
        assert!(Registry::db_path_for(&base, "F0000001").exists());
        assert_eq!(Registry::db_path_for(&base, "F0000001").file_name().unwrap(), "edda.F0000001.sqlite3");
        assert!(reg.store(PLACEHOLDER).is_none(), "the placeholder goes once a commander exists");
    }

    #[test]
    fn menu_only_files_open_no_store_and_pending_files_wait() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T190000.01.log"), MENU).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T191000.01.log"), concat!(r#"{"timestamp":"2026-10-17T19:10:00Z","event":"Fileheader","part":1}"#, "\n")).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        let out = reg.sync_all(|_, _, _| {}).unwrap();
        assert!(reg.fids().is_empty());
        assert_eq!(reg.keys(), vec![PLACEHOLDER.to_string()]);
        assert_eq!((out.menu_only_files, out.pending_files), (1, 1));
        // The pending file gets its Commander line: it is picked up on the next sync.
        std::fs::write(journal.join("Journal.2026-10-17T191000.01.log"), format!("{{\"timestamp\":\"2026-10-17T19:10:00Z\",\"event\":\"Fileheader\",\"part\":1}}\n{CMDR_A}\n{LOAD_A}\n")).unwrap();
        let out = reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(out.new_fids, vec!["F0000001".to_string()]);
        assert_eq!(count(&reg.store("F0000001").unwrap(), "SELECT COUNT(*) FROM events"), 3);
    }

    #[test]
    fn companions_go_to_the_store_whose_journal_they_match() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n{CARGO_A}\n")).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}\n{LOAD_B}\n{CARGO_B}\n")).unwrap();
        // B's Status (balance near 9 B) and B's Cargo (700 t); a Market nobody docked at.
        std::fs::write(journal.join("Status.json"), r#"{"timestamp":"2026-10-17T18:09:00Z","event":"Status","Balance":9000012345,"Fuel":{"FuelMain":30.0}}"#).unwrap();
        std::fs::write(journal.join("Cargo.json"), r#"{"timestamp":"2026-10-17T18:09:00Z","event":"Cargo","Vessel":"Ship","Count":700,"Inventory":[{"Name":"gold","Count":700}]}"#).unwrap();
        std::fs::write(journal.join("Market.json"), r#"{"timestamp":"2026-10-17T18:09:00Z","event":"Market","MarketID":3700000009,"Items":[]}"#).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        let out = reg.sync_all(|_, _, _| {}).unwrap();
        let a = reg.store("F0000001").unwrap();
        let b = reg.store("F0000002").unwrap();
        let fuel = |s: &Arc<Mutex<Store>>| -> Option<f64> { s.lock().unwrap().conn().query_row("SELECT json_extract(raw,'$.Fuel.FuelMain') FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).ok().flatten() };
        assert_eq!(fuel(&b), Some(30.0));
        assert_eq!(fuel(&a), None, "A never sees B's fuel");
        assert_eq!(count(&b, "SELECT COUNT(*) FROM cargo"), 1, "B's cargo table derived from B's Cargo.json");
        assert_eq!(count(&a, "SELECT COUNT(*) FROM cargo"), 0);
        let market_raw = |s: &Arc<Mutex<Store>>| -> Option<String> { s.lock().unwrap().conn().query_row("SELECT raw FROM snapshots WHERE name='Market.json'", [], |r| r.get(0)).ok() };
        assert_eq!(market_raw(&a).as_deref(), Some(crate::ingest::UNKNOWN_SNAPSHOT));
        assert_eq!(market_raw(&b).as_deref(), Some(crate::ingest::UNKNOWN_SNAPSHOT));
        let att: std::collections::HashMap<_, _> = out.companions.into_iter().collect();
        assert_eq!(att["Status.json"], Attribution::To(vec!["F0000002".to_string()]));
        assert_eq!(att["Cargo.json"], Attribution::To(vec!["F0000002".to_string()]));
        assert_eq!(att["Market.json"], Attribution::Unknown);
    }

    #[test]
    fn a_single_commander_gets_every_companion_as_today() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n")).unwrap();
        std::fs::write(journal.join("Market.json"), r#"{"timestamp":"2026-10-17T18:09:00Z","event":"Market","MarketID":3700000009,"Items":[]}"#).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        reg.sync_all(|_, _, _| {}).unwrap();
        let a = reg.store("F0000001").unwrap();
        let id: Option<i64> = a.lock().unwrap().conn().query_row("SELECT json_extract(raw,'$.MarketID') FROM snapshots WHERE name='Market.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(id, Some(3700000009));
    }

    #[test]
    fn legacy_store_with_one_fid_is_renamed_in_place() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n")).unwrap();
        {
            let legacy = Store::open(&base, &journal).unwrap();
            legacy.sync().unwrap();
            legacy.conn().execute("INSERT INTO user_marks (label, system, created_ts) VALUES ('Iridium', 'Sol', '2026-10-01T00:00:00Z')", []).unwrap();
        }
        let reg = Registry::open(&base, &journal).unwrap();
        assert!(!base.exists(), "edda.sqlite3 became edda.F0000001.sqlite3");
        assert!(Registry::db_path_for(&base, "F0000001").exists());
        assert_eq!(reg.fids(), vec!["F0000001".to_string()]);
        let a = reg.store("F0000001").unwrap();
        assert_eq!(count(&a, "SELECT COUNT(*) FROM user_marks"), 1, "bookmarks survive");
        assert_eq!(count(&a, "SELECT COUNT(*) FROM events"), 2, "no re-ingest needed");
        // Reopening is idempotent.
        drop(reg);
        let reg = Registry::open(&base, &journal).unwrap();
        assert_eq!(reg.fids(), vec!["F0000001".to_string()]);
    }

    #[test]
    fn legacy_store_with_two_fids_is_split_and_bookmarks_are_copied_to_both() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n")).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}\n{LOAD_B}\n")).unwrap();
        {
            let legacy = Store::open(&base, &journal).unwrap();
            legacy.sync().unwrap();
            legacy.conn().execute("INSERT INTO user_marks (label, system, created_ts) VALUES ('Iridium', 'Sol', '2026-10-01T00:00:00Z')", []).unwrap();
        }
        let reg = Registry::open(&base, &journal).unwrap();
        assert!(!base.exists());
        assert!(base.with_file_name("edda.blended.sqlite3").exists(), "the blended store is kept aside, not deleted");
        reg.sync_all(|_, _, _| {}).unwrap();
        for fid in ["F0000001", "F0000002"] {
            let s = reg.store(fid).unwrap();
            assert_eq!(count(&s, "SELECT COUNT(*) FROM user_marks"), 1, "{fid}");
            assert_eq!(count(&s, "SELECT COUNT(*) FROM events"), 2, "{fid}");
        }
    }

    #[test]
    fn last_writer_is_the_owner_of_the_newest_file() {
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}\n{LOAD_B}\n")).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(reg.last_writer(), Some("F0000002".to_string()));
        let (_d2, journal2, base2) = folder();
        let empty = Registry::open(&base2, &journal2).unwrap();
        assert_eq!(empty.last_writer(), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-store registry`
Expected: FAIL to compile (after adding `pub mod registry;` to lib.rs: `cannot find type Registry`, `Attribution`, `PLACEHOLDER`).

- [ ] **Step 3: Implement the Store additions**

In `crates/ed-store/src/lib.rs`:

```rust
pub struct Store {
    conn: Connection,
    db_path: Option<PathBuf>,
    journal_dir: PathBuf,
    /// The commander this store belongs to; `None` for the single-store
    /// path (examples, tests, a fresh install before any journal).
    fid: Option<String>,
    observed: std::sync::Mutex<merit_capture::CaptureState>,
}
```

Replace `open` with:

```rust
    pub fn open(db_path: &Path, journal_dir: &Path) -> Result<Self> {
        Self::open_for(db_path, journal_dir, None)
    }

    /// Open the store of one commander. `fid` is checked on ingest: a
    /// journal line naming another FID is refused (ingest::ingest_file_for).
    pub fn open_for(db_path: &Path, journal_dir: &Path, fid: Option<&str>) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("opening database at {}", db_path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        schema::migrate(&conn)?;
        schema::attach_galaxy(&conn, Some(&schema::galaxy_path(db_path)))?;
        Ok(Store {
            conn,
            db_path: Some(db_path.to_path_buf()),
            journal_dir: journal_dir.to_path_buf(),
            fid: fid.map(str::to_string),
            observed: std::sync::Mutex::new(merit_capture::CaptureState::new(observe::seen_award_keys())),
        })
    }

    pub fn fid(&self) -> Option<&str> {
        self.fid.as_deref()
    }

    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }
```

Add `fid: None,` to `open_in_memory`'s struct literal. Split `sync_with_progress`:

```rust
    pub fn sync_with_progress(&self, progress: impl FnMut(&str, usize, usize)) -> Result<SyncStats> {
        let started = std::time::Instant::now();
        let ingest = ingest::ingest_all(&self.conn, &self.journal_dir, progress)?;
        self.finish_sync(ingest, started)
    }

    /// Ingest only the journal files this store owns; no companions, no
    /// derive. The registry dispatches companions and calls `finish_sync`.
    pub fn ingest_owned(&self, names: &[String], progress: impl FnMut(&str, usize, usize)) -> Result<ingest::IngestStats> {
        ingest::ingest_files(&self.conn, &self.journal_dir, names, self.fid.as_deref(), progress)
    }

    /// Derive and capture after an ingest; the second half of a sync.
    pub fn finish_sync(&self, ingest: ingest::IngestStats, started: std::time::Instant) -> Result<SyncStats> {
        let derive = derive::derive_incremental(&self.conn)?;
        let observed = {
            let mut state = self.observed.lock().unwrap_or_else(|e| e.into_inner());
            merit_capture::capture(&self.conn, &mut state)
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        // (keep the existing trace! lines here, unchanged)
        Ok(SyncStats { ingest, derive, elapsed_ms })
    }
```

Add `pub mod companions;` (if Task 3 did not) and `pub mod registry;` to the module list.

- [ ] **Step 4: Implement the registry**

Fill `crates/ed-store/src/registry.rs` above the tests:

```rust
//! One store per commander. Two accounts in one Saved Games folder write
//! interleaved journal files and the same companion files; a single
//! `events` table with "newest row wins" readers showed a blend that
//! changed commander 263 times in 90 days on the donor data
//! (`docs/benches/2026-10-24-two-account-folder-baseline.csv`). Here each
//! journal file goes to the store of the FID its first Commander line
//! names, and each companion file to the store whose journal it matches.
use crate::ingest::{self, IngestStats};
use crate::{Store, SyncStats};
use anyhow::{Context as _, Result};
use ed_journal::journal_file::{owner, Owner};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The key of the FID-less store a fresh install runs on until the first
/// Commander line arrives (today's `edda.sqlite3`).
pub const PLACEHOLDER: &str = "";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    To(Vec<String>),
    Unknown,
}

#[derive(Debug, Default)]
pub struct RegistrySync {
    pub per_store: BTreeMap<String, SyncStats>,
    pub new_fids: Vec<String>,
    pub companions: Vec<(String, Attribution)>,
    pub menu_only_files: usize,
    pub pending_files: usize,
    pub elapsed_ms: u64,
}

struct Inner {
    stores: BTreeMap<String, Arc<Mutex<Store>>>,
    owners: HashMap<String, Owner>,
}

pub struct Registry {
    base: PathBuf,
    journal_dir: PathBuf,
    inner: Mutex<Inner>,
}

impl Registry {
    /// `base` is today's database path (`.../edda.sqlite3`); per-commander
    /// stores are its siblings. A legacy single-commander file is renamed
    /// once; a blended one is split (bookmarks copied) and kept aside.
    pub fn open(base: &Path, journal_dir: &Path) -> Result<Registry> {
        if let Some(parent) = base.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        migrate_legacy(base, journal_dir)?;
        let mut stores = BTreeMap::new();
        for fid in existing_fids(base)? {
            let store = Store::open_for(&Self::db_path_for(base, &fid), journal_dir, Some(&fid))?;
            stores.insert(fid, Arc::new(Mutex::new(store)));
        }
        if stores.is_empty() {
            stores.insert(PLACEHOLDER.to_string(), Arc::new(Mutex::new(Store::open_for(base, journal_dir, None)?)));
        }
        Ok(Registry {
            base: base.to_path_buf(),
            journal_dir: journal_dir.to_path_buf(),
            inner: Mutex::new(Inner { stores, owners: HashMap::new() }),
        })
    }

    pub fn db_path_for(base: &Path, fid: &str) -> PathBuf {
        if fid.is_empty() {
            base.to_path_buf()
        } else {
            base.with_file_name(format!("edda.{fid}.sqlite3"))
        }
    }

    pub fn journal_dir(&self) -> &Path {
        &self.journal_dir
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn fids(&self) -> Vec<String> {
        self.inner().stores.keys().filter(|k| !k.is_empty()).cloned().collect()
    }

    pub fn keys(&self) -> Vec<String> {
        self.inner().stores.keys().cloned().collect()
    }

    pub fn store(&self, key: &str) -> Option<Arc<Mutex<Store>>> {
        self.inner().stores.get(key).cloned()
    }

    pub fn stores(&self) -> Vec<(String, Arc<Mutex<Store>>)> {
        self.inner().stores.iter().map(|(k, s)| (k.clone(), s.clone())).collect()
    }

    /// The commander whose journal file was modified most recently.
    pub fn last_writer(&self) -> Option<String> {
        let inner = self.inner();
        let mut best: Option<(std::time::SystemTime, String)> = None;
        for (name, owner) in &inner.owners {
            let Owner::Fid(fid) = owner else { continue };
            let Ok(md) = std::fs::metadata(self.journal_dir.join(name)) else { continue };
            let Ok(m) = md.modified() else { continue };
            if best.as_ref().is_none_or(|(t, _)| m > *t) {
                best = Some((m, fid.clone()));
            }
        }
        best.map(|(_, f)| f)
    }

    /// Attribute new files, ingest each store's own, dispatch the
    /// companions by content, derive. `progress` is called per file as
    /// `Store::sync_with_progress` does.
    pub fn sync_all(&self, mut progress: impl FnMut(&str, usize, usize)) -> Result<RegistrySync> {
        let started = std::time::Instant::now();
        let mut out = RegistrySync::default();
        let names = ingest::journal_file_names(&self.journal_dir)?;

        // 1. Owners: new files and files still pending.
        let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let mut inner = self.inner();
            for name in &names {
                let known = inner.owners.get(name).cloned();
                let o = match known {
                    Some(Owner::Fid(f)) => Owner::Fid(f),
                    Some(Owner::MenuOnly) => Owner::MenuOnly,
                    _ => {
                        let o = owner(&self.journal_dir.join(name)).unwrap_or(Owner::Pending);
                        inner.owners.insert(name.clone(), o.clone());
                        o
                    }
                };
                match o {
                    Owner::Fid(fid) => {
                        if !inner.stores.contains_key(&fid) {
                            let store = Store::open_for(&Self::db_path_for(&self.base, &fid), &self.journal_dir, Some(&fid))?;
                            inner.stores.insert(fid.clone(), Arc::new(Mutex::new(store)));
                            out.new_fids.push(fid.clone());
                            tracing::info!(%fid, "new commander: store opened");
                        }
                        owned.entry(fid).or_default().push(name.clone());
                    }
                    Owner::MenuOnly => out.menu_only_files += 1,
                    Owner::Pending => out.pending_files += 1,
                }
            }
            // The placeholder retires once a real commander exists and it
            // holds nothing (a fresh install's first launch).
            if inner.stores.len() > 1 && inner.stores.contains_key(PLACEHOLDER) {
                let empty = inner
                    .stores
                    .get(PLACEHOLDER)
                    .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).event_count().unwrap_or(1) == 0)
                    .unwrap_or(false);
                if empty {
                    inner.stores.remove(PLACEHOLDER);
                    tracing::info!("placeholder store retired");
                }
            }
        }
        if out.menu_only_files > 0 {
            tracing::debug!(files = out.menu_only_files, "menu-only journal files ignored");
        }

        let stores = self.stores();
        // A FID-less install syncs exactly as today.
        if stores.len() == 1 && stores[0].0 == PLACEHOLDER {
            let stats = stores[0].1.lock().unwrap_or_else(|e| e.into_inner()).sync_with_progress(progress)?;
            out.per_store.insert(PLACEHOLDER.to_string(), stats);
            out.elapsed_ms = started.elapsed().as_millis() as u64;
            return Ok(out);
        }

        // 2. Each store ingests its own files.
        let total: usize = owned.values().map(Vec::len).sum();
        let mut done = 0usize;
        let mut ingested: BTreeMap<String, IngestStats> = BTreeMap::new();
        for (fid, store) in &stores {
            let mine = owned.get(fid).cloned().unwrap_or_default();
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            let stats = guard.ingest_owned(&mine, |name, _, _| {
                done += 1;
                progress(name, done, total);
            })?;
            ingested.insert(fid.clone(), stats);
        }

        // 3. Companions by content.
        for name in ingest::COMPANION_FILES {
            let path = self.journal_dir.join(name);
            let Ok(raw) = std::fs::read_to_string(&path) else { continue };
            let raw = raw.trim().trim_start_matches('\u{feff}').to_string();
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
            let ts = v.get("timestamp").and_then(|x| x.as_str());
            let mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
            let mut scores = Vec::new();
            for (fid, store) in &stores {
                let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                scores.push((fid.clone(), crate::companions::score(guard.conn(), name, &v)?));
            }
            let chosen = crate::companions::choose(&scores);
            for (fid, store) in &stores {
                let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                // Unchanged since the stored copy: nothing to do (same mtime rule as ingest_companions).
                let stored: Option<Option<i64>> = guard.conn().query_row("SELECT mtime FROM snapshots WHERE name = ?1", [name], |r| r.get(0)).ok();
                if chosen.iter().any(|c| c == fid) {
                    if stored != Some(mtime) {
                        ingest::write_snapshot(guard.conn(), name, ts, mtime, &raw)?;
                        if let Some(s) = ingested.get_mut(fid) { s.snapshots_updated += 1; }
                    }
                } else if chosen.is_empty() {
                    if stored != Some(None) {
                        ingest::mark_unknown(guard.conn(), name)?;
                        if let Some(s) = ingested.get_mut(fid) { s.snapshots_updated += 1; }
                    }
                }
                // Chosen by another store: this store keeps its own last known copy.
            }
            let att = if chosen.is_empty() { Attribution::Unknown } else { Attribution::To(chosen) };
            if att == Attribution::Unknown {
                tracing::debug!(file = name, ?scores, "companion file attributed to nobody");
            }
            out.companions.push(((*name).to_string(), att));
        }

        // 4. Derive and capture.
        for (fid, store) in &stores {
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            let stats = guard.finish_sync(ingested.remove(fid).unwrap_or_default(), started)?;
            out.per_store.insert(fid.clone(), stats);
        }
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(out)
    }
}

/// FIDs that already have a store file beside `base`.
fn existing_fids(base: &Path) -> Result<Vec<String>> {
    let Some(dir) = base.parent() else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(fid) = name.strip_prefix("edda.").and_then(|n| n.strip_suffix(".sqlite3")) {
                if fid.starts_with('F') && fid[1..].chars().all(|c| c.is_ascii_digit()) {
                    out.push(fid.to_string());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The legacy `edda.sqlite3`: one commander's store is renamed to its FID;
/// a blended store is split (bookmarks to every commander) and kept aside
/// as `edda.blended.sqlite3`; a store with no commander stays as the
/// placeholder.
fn migrate_legacy(base: &Path, journal_dir: &Path) -> Result<()> {
    if !base.exists() || !existing_fids(base)?.is_empty() {
        return Ok(());
    }
    let fids: Vec<String> = {
        let conn = rusqlite::Connection::open(base).with_context(|| format!("opening legacy store {}", base.display()))?;
        crate::schema::migrate(&conn)?;
        let mut stmt = conn.prepare("SELECT DISTINCT json_extract(raw, '$.FID') FROM events WHERE event = 'Commander' AND json_extract(raw, '$.FID') IS NOT NULL ORDER BY 1")?;
        let fids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        fids
    };
    let sidecars = |p: &Path| {
        for ext in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(PathBuf::from(format!("{}{ext}", p.display())));
        }
    };
    match fids.as_slice() {
        [] => Ok(()),
        [fid] => {
            let target = Registry::db_path_for(base, fid);
            std::fs::rename(base, &target).with_context(|| format!("renaming {} to {}", base.display(), target.display()))?;
            sidecars(base);
            tracing::info!(%fid, "legacy store renamed to its commander");
            Ok(())
        }
        many => {
            let blended = base.with_file_name("edda.blended.sqlite3");
            std::fs::rename(base, &blended)?;
            sidecars(base);
            for fid in many {
                let store = Store::open_for(&Registry::db_path_for(base, fid), journal_dir, Some(fid))?;
                let conn = store.conn();
                conn.execute("ATTACH DATABASE ?1 AS legacy", [blended.to_string_lossy().as_ref()])?;
                conn.execute_batch("INSERT INTO user_marks SELECT * FROM legacy.user_marks; DETACH DATABASE legacy;")?;
            }
            tracing::warn!(commanders = many.len(), kept = %blended.display(), "legacy store held two commanders; split, bookmarks copied to each, the journal re-ingests");
            Ok(())
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p ed-store`
Expected: PASS, the 7 registry tests included. Known traps: `Store::event_count` exists (lib.rs:231); `is_none_or` needs Rust 1.82+ (the workspace is newer). If the blended test fails on `ATTACH` with a Windows path, pass the path through `.replace('\\', "/")`.

- [ ] **Step 6: Run the examples still compile**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo build -p ed-store --examples`
Expected: builds (the examples use `Store::open`, `sync_with_progress`, `ingest_all`, all kept).

- [ ] **Step 7: Commit**

```bash
git add crates/ed-store/src/lib.rs crates/ed-store/src/registry.rs
git commit -m "Multiple commanders: the registry -- one store per FID (edda.<FID>.sqlite3 beside edda.sqlite3, galaxy shared), journal files attributed by their first Commander line and ingested only into their owner's store, companion files scored by content against every store and written to the chosen one (unknown = '{}' everywhere), menu-only files ignored, pending files re-read; a legacy single-commander store is renamed once (bookmarks and events intact), a blended one split with bookmarks copied to each and kept aside; Store::open_for/ingest_owned/finish_sync; the FID-less placeholder syncs as today; tests RED first"
```

---

### Task 5: The replay harness and its verdict

**Files:**
- Create: `crates/ed-store/examples/multibox_replay.rs`
- Create: `docs/benches/2026-10-24-multibox-replay.csv`
- Modify: `docs/benches/knobs/multibox/README.md`

**Interfaces:**
- Consumes: `Registry::{open, sync_all, fids, stores, last_writer}` (Task 4).
- Produces: a CSV record. No code interface.

Pre-registered (write these into the CSV header BEFORE running): the donor folder replays into exactly 2 stores; each store's `Commander` events carry exactly one distinct FID; 164 menu-only files; Status.json, Cargo.json attributed to one store or unknown, never to the wrong one (checked by Balance: the chosen store's credits estimate is the nearer); total events across stores = 1,257,243 minus the 732 menu-only lines; wall within 1.3x of the single-store 150 s; three quiet `sync_all` passes each under 1.1x the single-store quiet pass (80 ms ingest + 698 ms derive measured 2026-10-24).

- [ ] **Step 1: Write the harness**

```rust
//! Replay a journal folder through the registry: one store per commander.
//!
//!     cargo run -p ed-store --release --example multibox_replay -- <journal_dir> <scratch_dir>
//!
//! Prints counts and labels only: no name, FID, system or balance from the
//! folder is printed (FIDs are shown as A, B, ... in first-seen order).
//! Record: docs/benches/2026-10-24-multibox-replay.csv.
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let journal = PathBuf::from(args.next().expect("usage: multibox_replay <journal_dir> <scratch_dir>"));
    let scratch = PathBuf::from(args.next().expect("usage: multibox_replay <journal_dir> <scratch_dir>"));
    std::fs::create_dir_all(&scratch)?;
    let base = scratch.join("edda.sqlite3");

    let t0 = Instant::now();
    let reg = ed_store::registry::Registry::open(&base, &journal)?;
    let open_ms = t0.elapsed().as_millis();
    let t1 = Instant::now();
    let mut last = Instant::now();
    let out = reg.sync_all(|name, i, total| {
        if last.elapsed().as_secs() >= 30 || i == total {
            last = Instant::now();
            println!("  [{i:>4}/{total}] {name} at {:.1} s", t1.elapsed().as_secs_f64());
        }
    })?;
    let sync_s = t1.elapsed().as_secs_f64();

    let fids = reg.fids();
    let label = |fid: &str| -> String { fids.iter().position(|f| f == fid).map(|i| ((b'A' + i as u8) as char).to_string()).unwrap_or_else(|| "?".into()) };
    println!("\n─ registry ───────────────────────────────");
    println!("  open             {open_ms} ms");
    println!("  sync_all         {sync_s:.1} s");
    println!("  commanders       {}", fids.len());
    println!("  new_fids         {}", out.new_fids.len());
    println!("  menu_only_files  {}", out.menu_only_files);
    println!("  pending_files    {}", out.pending_files);
    println!("  last_writer      {}", reg.last_writer().as_deref().map(label).unwrap_or_default());
    let mut total_events = 0i64;
    let mut purity_ok = true;
    for (fid, store) in reg.stores() {
        let g = store.lock().unwrap();
        let conn = g.conn();
        let events: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        let distinct: i64 = conn.query_row("SELECT COUNT(DISTINCT json_extract(raw,'$.FID')) FROM events WHERE event IN ('Commander','LoadGame') AND json_extract(raw,'$.FID') IS NOT NULL", [], |r| r.get(0))?;
        let files: i64 = conn.query_row("SELECT COUNT(*) FROM journal_files", [], |r| r.get(0))?;
        let credits = ed_store::session::credits_estimate(conn)?.map(|_| "known").unwrap_or("none");
        let bytes = std::fs::metadata(ed_store::registry::Registry::db_path_for(&base, &fid)).map(|m| m.len()).unwrap_or(0);
        if distinct != 1 { purity_ok = false; }
        println!("  store {}: events {events}, files {files}, distinct FIDs {distinct}, credits estimate {credits}, {:.1} MB", label(&fid), bytes as f64 / 1e6);
        total_events += events;
    }
    println!("  total events     {total_events}");
    println!("  purity           {}", if purity_ok { "ok: one FID per store" } else { "FAILED: a store holds two FIDs" });
    println!("\n─ companions ─────────────────────────────");
    for (name, att) in &out.companions {
        let to = match att {
            ed_store::registry::Attribution::To(v) => v.iter().map(|f| label(f)).collect::<Vec<_>>().join("+"),
            ed_store::registry::Attribution::Unknown => "unknown".into(),
        };
        println!("  {name:<18} -> {to}");
    }

    println!("\n─ quiet sync_all x3 ──────────────────────");
    for pass in 1..=3 {
        let t = Instant::now();
        let q = reg.sync_all(|_, _, _| {})?;
        let inserted: u64 = q.per_store.values().map(|s| s.ingest.events_inserted).sum();
        println!("  pass {pass}: {} ms (events inserted {inserted}, snapshots {})", t.elapsed().as_millis(), q.per_store.values().map(|s| s.ingest.snapshots_updated).sum::<usize>());
    }
    Ok(())
}
```

- [ ] **Step 2: Build it**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo build -p ed-store --release --example multibox_replay`
Expected: builds.

- [ ] **Step 3: Write the CSV header with the pre-registration, then run**

Create `docs/benches/2026-10-24-multibox-replay.csv` with these header lines (fill the verdict AFTER the run; leave `# Verdict: PENDING` until then):

```
# Multibox replay: the two-account journal folder (1,396 files, 2025-01 .. 2026-10) through the registry, 2026-10-24. Knob: cargo run -p ed-store --release --example multibox_replay -- <folder> <scratch>; baseline: docs/benches/2026-10-24-two-account-folder-baseline.csv.
# Pre-registered: 2 stores; one distinct FID per store (purity); 164 menu-only files, 0 pending; Status.json and Cargo.json attributed to one store (the one whose credits estimate is nearer the file's Balance) or unknown, never the other; events across stores = 1,257,243 - 732; wall <= 1.3 x 150 s; each quiet pass <= 1.1 x (80 + 698) ms.
# Verdict: PENDING
metric,value,note
```

Run: `"C:\Users\terak\ED-Claude\edda-target-limits\release\examples\multibox_replay.exe" "C:\Users\terak\Downloads\Elite Dangerous\Elite Dangerous" "C:\Users\terak\AppData\Local\Temp\claude\C--Users-terak-ED-Claude-edda-app\e72c0b08-1d04-4299-9db2-de14f25fbd6a\scratchpad\multibox\replay"`
Expected: `commanders 2`, `purity ok`, `menu_only_files 164`, companions listed, three quiet passes printed. Copy the numbers into the CSV as `metric,value,note` rows (open_ms, sync_s, commanders, menu_only_files, pending_files, events_A, events_B, total_events, purity, each companion's attribution as a label, quiet_pass_1..3_ms) and replace `# Verdict: PENDING` with the verdict against each pre-registered line. A miss is recorded as a miss, in the header, with its number.

- [ ] **Step 4: Add the knob to the README**

Append to `docs/benches/knobs/multibox/README.md`:

```
- `cargo run -p ed-store --release --example multibox_replay -- <folder> <scratch>` -- the
  folder through the registry: stores, purity, companion attribution, quiet-sync cost.
  Record: `docs/benches/2026-10-24-multibox-replay.csv`.
```

- [ ] **Step 5: Commit**

```bash
git add crates/ed-store/examples/multibox_replay.rs docs/benches/2026-10-24-multibox-replay.csv docs/benches/knobs/multibox/README.md
git commit -m "Multiple commanders: the replay harness and its verdict -- the two-account folder through the registry: <N> stores, one FID per store, <menu> menu-only files ignored, companions attributed <summary>, <wall> s against 150 s single-store, quiet passes <ms> ms; pre-registered in the CSV header before the run"
```

(Replace the angle-bracket figures with the measured ones.)

---

### Task 6: The app over the registry: selection, config, commands, Frontier link

**Files:**
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/capi.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/state.rs` `mod commander_tests`, `src-tauri/src/capi.rs` tests

**Interfaces:**
- Consumes: `ed_store::registry::{Registry, PLACEHOLDER}`, `Store::db_path()` (Task 4).
- Produces:
  ```rust
  // state.rs
  pub struct AppConfig { .., #[serde(default)] pub selected_commander: Option<String>, #[serde(default)] pub other_commander_alerts: bool }
  pub struct AppState { pub commanders: Arc<Registry>, selected: Mutex<String>, readers: Mutex<(String, Vec<Connection>)>, .. }   // `store` and `db_path` fields removed
  impl AppState {
      pub fn new(commanders: Arc<Registry>, data_dir: PathBuf) -> Self
      pub fn selected_fid(&self) -> String
      pub fn selected_store(&self) -> Arc<Mutex<Store>>
      pub fn select(&self, fid: &str) -> Result<(), String>       // validates against commanders.keys(); sets; drops readers; clears dock_memo; saves config
      pub fn db_path(&self) -> PathBuf                            // the selected store's file
      pub fn adopt_new_commanders(&self, new_fids: &[String])      // if selected is PLACEHOLDER or gone, select the first
      pub fn initial_selection(config: &AppConfig, commanders: &Registry) -> String
  }
  // commands.rs
  #[derive(serde::Serialize)] pub struct CommanderRow { pub fid: String, pub name: Option<String>, pub ship: Option<String>, pub system: Option<String>, pub selected: bool, pub wrote_s_ago: Option<i64>, pub store_bytes: u64 }
  pub fn commander_rows(state: &AppState) -> Vec<CommanderRow>
  #[tauri::command] pub async fn list_commanders(state) -> Result<Vec<CommanderRow>, String>
  #[tauri::command] pub async fn select_commander(state, fid: String) -> Result<Vec<CommanderRow>, String>
  #[tauri::command] pub async fn other_commander_alerts(state) -> Result<bool, String>
  #[tauri::command] pub async fn other_commander_alerts_set(state, enabled: bool) -> Result<(), String>
  // capi.rs
  pub fn refresh_slot(fid: &str) -> String   // "" → REFRESH_SLOT; else format!("{REFRESH_SLOT}:{fid}")
  pub fn migrate_legacy_slot(fid: &str)       // move REFRESH_SLOT's value to refresh_slot(fid) when the latter is empty
  ```

- [ ] **Step 1: Write the failing tests**

Add to `src-tauri/src/state.rs`:

```rust
#[cfg(test)]
mod commander_tests {
    use super::*;

    fn registry_with(fids: &[&str]) -> (tempfile::TempDir, std::sync::Arc<ed_store::registry::Registry>) {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        for (i, fid) in fids.iter().enumerate() {
            std::fs::write(
                journal.join(format!("Journal.2026-10-17T18{i:02}000.01.log")),
                format!("{{\"timestamp\":\"2026-10-17T18:{i:02}:00Z\",\"event\":\"Commander\",\"FID\":\"{fid}\",\"Name\":\"C{i}\"}}\n"),
            )
            .unwrap();
        }
        let reg = ed_store::registry::Registry::open(&dir.path().join("edda.sqlite3"), &journal).unwrap();
        reg.sync_all(|_, _, _| {}).unwrap();
        (dir, std::sync::Arc::new(reg))
    }

    #[test]
    fn initial_selection_prefers_the_saved_commander_then_the_last_writer_then_the_first() {
        let (_d, reg) = registry_with(&["F0000001", "F0000002"]);
        let mut cfg = AppConfig::default();
        cfg.selected_commander = Some("F0000002".into());
        assert_eq!(AppState::initial_selection(&cfg, &reg), "F0000002");
        cfg.selected_commander = Some("F9999999".into());
        let fallback = AppState::initial_selection(&cfg, &reg);
        assert!(reg.fids().contains(&fallback), "a saved commander that is gone falls back to a real one");
        let (_d2, empty) = registry_with(&[]);
        assert_eq!(AppState::initial_selection(&AppConfig::default(), &empty), ed_store::registry::PLACEHOLDER);
    }

    #[test]
    fn config_round_trips_the_new_fields_and_defaults_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.selected_commander, None);
        assert!(!cfg.other_commander_alerts);
        cfg.selected_commander = Some("F0000001".into());
        cfg.other_commander_alerts = true;
        cfg.save(dir.path()).unwrap();
        let back = AppConfig::load(dir.path());
        assert_eq!(back.selected_commander.as_deref(), Some("F0000001"));
        assert!(back.other_commander_alerts);
        // An old config.json without the fields loads with the defaults.
        std::fs::write(dir.path().join("config.json"), r#"{"voice_muted":true}"#).unwrap();
        let old = AppConfig::load(dir.path());
        assert_eq!(old.selected_commander, None);
        assert!(!old.other_commander_alerts);
    }
}
```

Add to `src-tauri/src/capi.rs` tests (there is a `#[cfg(test)] mod tests` with `fid_matches` tests; add beside them):

```rust
    #[test]
    fn the_refresh_slot_is_per_commander_and_the_legacy_slot_for_none() {
        assert_eq!(refresh_slot(""), REFRESH_SLOT);
        assert_eq!(refresh_slot("F0000001"), "frontier_refresh_token:F0000001");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p edda commander_tests refresh_slot`
Expected: FAIL to compile: no `selected_commander`, `other_commander_alerts`, `initial_selection`, `refresh_slot`. (`tempfile` must be a dev-dependency of src-tauri; if `cargo test -p edda` says it is not, add `tempfile = "3.27.0"` under `[dev-dependencies]` in `src-tauri/Cargo.toml`.)

- [ ] **Step 3: Implement the config and state**

In `src-tauri/src/state.rs`, add to `AppConfig` after `speculative_missions`:

```rust
    /// The commander the panels, HUD and voice follow when the journal
    /// folder holds more than one (maintainer, 2026-10-24: manual
    /// selection first). A FID; `None` means the last writer at launch.
    #[serde(default)]
    pub selected_commander: Option<String>,
    /// Speak the danger set for the commanders NOT selected (under
    /// attack, interdicted, overheating, destroyed, carrier jump
    /// scheduled), prefixed with their name. Off by default.
    #[serde(default)]
    pub other_commander_alerts: bool,
```

Change `AppState`:

```rust
pub struct AppState {
    /// Every commander's store, one SQLite file each.
    pub commanders: Arc<ed_store::registry::Registry>,
    /// The FID the app follows (ed_store::registry::PLACEHOLDER before any).
    selected: Mutex<String>,
    ...
    /// Idle read-only connections to the SELECTED store, galaxy attached;
    /// dropped on a switch.
    pub readers: Mutex<(String, Vec<rusqlite::Connection>)>,
    // remove: pub store, pub db_path
}
```

Replace `AppState::new`'s signature with `pub fn new(commanders: Arc<ed_store::registry::Registry>, data_dir: PathBuf) -> Self`, drop the `store:` and `db_path:` struct fields, set `readers: Mutex::new((String::new(), Vec::new()))`, and after `config` is built set `selected: Mutex::new(Self::initial_selection(&config.lock().unwrap_or_else(|e| e.into_inner()), &commanders))` and `commanders`. Add the methods:

```rust
    pub fn initial_selection(config: &AppConfig, commanders: &ed_store::registry::Registry) -> String {
        let keys = commanders.keys();
        if let Some(saved) = config.selected_commander.as_deref() {
            if keys.iter().any(|k| k == saved) {
                return saved.to_string();
            }
        }
        if let Some(last) = commanders.last_writer() {
            if keys.iter().any(|k| *k == last) {
                return last;
            }
        }
        keys.into_iter().next().unwrap_or_else(|| ed_store::registry::PLACEHOLDER.to_string())
    }

    pub fn selected_fid(&self) -> String {
        self.selected.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn selected_store(&self) -> Arc<Mutex<Store>> {
        let fid = self.selected_fid();
        self.commanders.store(&fid).or_else(|| self.commanders.stores().into_iter().next().map(|(_, s)| s)).expect("the registry always holds a store")
    }

    /// The selected store's database file.
    pub fn db_path(&self) -> PathBuf {
        ed_store::registry::Registry::db_path_for(self.commanders.base(), &self.selected_fid())
    }

    /// Switch commanders: every later read goes to that store.
    pub fn select(&self, fid: &str) -> Result<(), String> {
        if self.commanders.store(fid).is_none() {
            return Err(format!("no commander {fid} in the journal folder"));
        }
        *self.selected.lock().unwrap_or_else(|e| e.into_inner()) = fid.to_string();
        self.readers.lock().unwrap_or_else(|e| e.into_inner()).1.clear();
        *self.dock_memo.lock().unwrap_or_else(|e| e.into_inner()) = crate::routing::DockMemo::default();
        let mut cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if cfg.selected_commander.as_deref() != Some(fid) {
            cfg.selected_commander = Some(fid.to_string());
            if let Err(error) = cfg.save(&self.data_dir) {
                tracing::warn!(%error, "commander selected but not remembered");
            }
        }
        tracing::info!(fid, "commander selected");
        Ok(())
    }

    /// A first Commander line arrived (fresh install) or the selected
    /// store vanished: follow the first real commander.
    pub fn adopt_new_commanders(&self, new_fids: &[String]) {
        let current = self.selected_fid();
        if (current == ed_store::registry::PLACEHOLDER || self.commanders.store(&current).is_none()) && !new_fids.is_empty() {
            let _ = self.select(&new_fids[0]);
        }
    }
```

Update `read_conn` to open `&self.db_path()` and attach `galaxy_path(&self.db_path())`. Update `with_read`: pop from the pool only when `pool.0 == self.selected_fid()`, else clear it first; push back under the selected FID. Update `with_store` to lock `self.selected_store()`. Update `with_read`'s fallback to `self.selected_store()`.

- [ ] **Step 4: Update the direct store sites**

- `src-tauri/src/commands.rs:3288` and `:3293` (`db_stats`): `state.db_path` → `state.db_path()`.
- `src-tauri/src/commands.rs` the one `state.store` site (grep `state.store` in commands.rs): `state.store.lock()` → `state.selected_store().lock()`.
- `src-tauri/src/capi.rs` the one `state.store` site: same replacement.
- `src-tauri/src/lib.rs:236-240`: replace the `Store::open` block with
  ```rust
  let commanders = match ed_store::registry::Registry::open(&db, &journal_dir) {
      Ok(r) => std::sync::Arc::new(r),
      Err(e) => {
          eprintln!("[edda] fatal: could not open the database: {e:#}");
          std::process::exit(1);
      }
  };
  tracing::info!(commanders = commanders.fids().len(), "stores opened");
  let app_state = AppState::new(commanders.clone(), data_dir);
  let registry_handle = commanders.clone();
  ```
  and remove `let store_handle = app_state.store.clone();`. The two `store_handle.clone()` uses (initial sync at ~349, watcher at ~354) become `registry_handle.clone()`; the watcher call is rewritten in Task 7; for now pass `registry_handle.clone()` to `initial_sync` and leave the watcher call compiling by giving it `app_state`-free arguments as Task 7 defines (do Task 7's signature change for `watcher::run` here only if needed to compile; otherwise leave the watcher untouched until Task 7 and pass `registry_handle.store(&registry_handle.keys()[0]).unwrap()` as a temporary store handle, with a `// Task 7 replaces this` comment).
- `initial_sync` (lib.rs:754): signature `fn initial_sync(app: &tauri::AppHandle, registry: &ed_store::registry::Registry, events: &dyn events::Emitter, announcer: &watcher::Announcer, fresh_database: bool)`; body: `let result = registry.sync_all(|name, i, total| { ..same progress emit.. });` then on `Ok(out)`: `let state = app.state::<AppState>(); state.adopt_new_commanders(&out.new_fids);` log `events = out.per_store.values().map(|s| s.ingest.events_inserted).sum::<u64>()`, emit `SYNC_COMPLETE` with that sum, run `follow::reconcile` on `state.selected_store()`'s conn, and `greet(announcer, &state.selected_store())` when `!fresh_database`. `greet`'s parameter type `&std::sync::Mutex<Store>` stays (pass `&*state.selected_store()`).
- After `AppState` is managed and before the watcher spawns in `setup`, add the keychain migration:
  ```rust
  if state.commanders.fids().len() == 1 {
      capi::migrate_legacy_slot(&state.selected_fid());
  }
  ```

- [ ] **Step 5: Implement the Frontier-link slot**

In `src-tauri/src/capi.rs`:

```rust
/// The keychain slot of one commander's refresh token. The legacy slot
/// (no FID) is what every install before multiple commanders used.
pub fn refresh_slot(fid: &str) -> String {
    if fid.is_empty() { REFRESH_SLOT.to_string() } else { format!("{REFRESH_SLOT}:{fid}") }
}

/// Move the legacy slot's token to the commander's slot, once.
pub fn migrate_legacy_slot(fid: &str) {
    if fid.is_empty() {
        return;
    }
    let slot = refresh_slot(fid);
    if secrets::get_key(&slot).is_some() {
        return;
    }
    if let Some(token) = secrets::get_key(REFRESH_SLOT) {
        if secrets::set_key(&slot, &token).is_ok() {
            let _ = secrets::clear_key(REFRESH_SLOT);
            tracing::info!("frontier link: refresh token moved to the commander's slot");
        }
    }
}
```

Replace the six `REFRESH_SLOT` uses at lines ~352, 444, 467, 522, 532, 660 with `&refresh_slot(&<state>.selected_fid())`, where `<state>` is the `AppState` each function already has (`state`, `st`, or `capi(app)`; `status(state)` has `state`, `unlink` has `st`). The `fid_matches` check keeps comparing against the journal FID read from the selected store.

- [ ] **Step 6: Implement the commands**

In `src-tauri/src/commands.rs`:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommanderRow {
    pub fid: String,
    pub name: Option<String>,
    pub ship: Option<String>,
    pub system: Option<String>,
    pub selected: bool,
    /// Seconds since this commander's journal last wrote, if ever.
    pub wrote_s_ago: Option<i64>,
    pub store_bytes: u64,
}

pub fn commander_rows(state: &AppState) -> Vec<CommanderRow> {
    let selected = state.selected_fid();
    let now = chrono::Utc::now();
    state
        .commanders
        .stores()
        .into_iter()
        .filter(|(fid, _)| !fid.is_empty())
        .map(|(fid, store)| {
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            let conn = guard.conn();
            let name = ed_store::session::commander_name(conn).ok().flatten();
            let ship = ed_store::session::current_ship(conn).ok().flatten().and_then(|s| s.spoken(ed_route::ships::display_name));
            let system = ed_store::query::location(conn).ok().flatten().and_then(|l| l.system_name);
            let wrote_s_ago = ed_store::session::last_event_key(conn).ok().flatten().and_then(|k| chrono::DateTime::parse_from_rfc3339(&k.ts).ok()).map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds());
            let store_bytes = sqlite_family_bytes(&ed_store::registry::Registry::db_path_for(state.commanders.base(), &fid));
            CommanderRow { selected: fid == selected, fid, name, ship, system, wrote_s_ago, store_bytes }
        })
        .collect()
}

#[tauri::command]
pub async fn list_commanders(state: State<'_, AppState>) -> Result<Vec<CommanderRow>, String> {
    Ok(commander_rows(&state))
}

/// Switch the panels, HUD and voice to another of the player's own
/// commanders. Every panel re-derives on the journal-changed signal.
#[tauri::command]
pub async fn select_commander(state: State<'_, AppState>, fid: String) -> Result<Vec<CommanderRow>, String> {
    state.select(&fid)?;
    state.events.emit(crate::events::JOURNAL_CHANGED, 0u64);
    Ok(commander_rows(&state))
}

#[tauri::command]
pub async fn other_commander_alerts(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.config.lock().unwrap_or_else(|e| e.into_inner()).other_commander_alerts)
}

#[tauri::command]
pub async fn other_commander_alerts_set(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    config.other_commander_alerts = enabled;
    config.save(&state.data_dir).map_err(|e| e.to_string())
}
```

`ed_store::session::current_ship(conn)` and `.spoken(display)` exist as `greet` uses them (lib.rs:804-807; `spoken` takes `impl Fn(&str) -> String`); `ed_store::query::location(conn)` returns `Result<Option<Location>>` with `system_name: Option<String>` (first_sync.rs uses `l.system_name.clone()`). `state.events.emit(event, payload)` is `events::EmitExt::emit<T: Serialize>` (events.rs:55); add `use crate::events::EmitExt as _;` at the top of commands.rs if it is not imported there. `sqlite_family_bytes` takes `&std::path::Path` (commands.rs:3273).

Register in `lib.rs`'s `generate_handler![`: `list_commanders, select_commander, other_commander_alerts, other_commander_alerts_set,` (with the `commands::` prefix if the list uses it for the neighbours).

Verify the frontend ignores the journal-changed payload before relying on `0u64`:
Run: `grep -rn "onJournalChanged" frontend/src --include=*.svelte --include=*.js -A3 | grep -i "payload"`
Expected: no output. (If a listener reads the payload, emit `1u64` instead and note it in the ledger.)

- [ ] **Step 7: Build and run the tests**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p edda`
Expected: compiles; `commander_tests` and `the_refresh_slot_is_per_commander_and_the_legacy_slot_for_none` PASS; every existing edda test still passes (`startup_without_journal_dir_reports_missing_rather_than_scanning_cwd` among them).

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/commands.rs src-tauri/src/capi.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "Multiple commanders: the app over the registry -- AppState holds every commander's store and a selected FID (saved commander, else the last journal writer, else the first), with_store and with_read serve the selected store so no query changes, the reader pool drops on a switch, list_commanders/select_commander/other_commander_alerts commands, the Frontier link's refresh token in a per-commander keychain slot with the legacy slot migrated once; tests RED first"
```

---

### Task 7: The watcher: one pass per store, danger only for the others

**Files:**
- Modify: `src-tauri/src/callouts.rs`
- Modify: `src-tauri/src/watcher.rs`
- Modify: `src-tauri/src/lib.rs` (the watcher spawn)
- Test: `callouts.rs` tests

**Interfaces:**
- Consumes: `AppState::{selected_fid, adopt_new_commanders, config}` (Task 6); `Registry::{sync_all, stores}` (Task 4).
- Produces:
  ```rust
  // callouts.rs
  pub fn is_for_other_commander(c: &Callout, event: Option<&serde_json::Value>) -> bool   // c.speak && (c.kind == "danger" || event is CarrierJumpRequest)
  pub fn prefix_for_other(name: Option<&str>, c: &mut Callout)                              // "Beta: Under attack."; "Other commander: ..." when the name is unknown
  // watcher.rs
  pub fn run(token: CancellationToken, app: AppHandle, registry: Arc<ed_store::registry::Registry>, journal_dir: &Path)
  ```

- [ ] **Step 1: Write the failing tests**

Add to `src-tauri/src/callouts.rs` tests module:

```rust
    #[test]
    fn other_commander_keeps_only_danger() {
        let ts = "2026-10-17T18:06:55Z";
        let attack = Callout::new("danger", ts, 3, true, "Under attack.".into());
        let fuel = Callout::new("fuel", ts, 2, true, "Fuel low.".into());
        let mission = Callout::new("mission", ts, 1, true, "Mission complete.".into());
        let silent_danger = Callout::new("danger", ts, 1, false, "Danger flag: hostile contacts nearby".into());
        let carrier = Callout::new("carrier", ts, 1, true, "Carrier jump to X scheduled, departure in 15 minutes.".into());
        let jump_req = serde_json::json!({"event":"CarrierJumpRequest"});
        let jump_done = serde_json::json!({"event":"CarrierJump"});
        assert!(is_for_other_commander(&attack, None));
        assert!(!is_for_other_commander(&fuel, None));
        assert!(!is_for_other_commander(&mission, None));
        assert!(!is_for_other_commander(&silent_danger, None), "a callout that is not spoken is not an alert");
        assert!(is_for_other_commander(&carrier, Some(&jump_req)));
        assert!(!is_for_other_commander(&carrier, Some(&jump_done)));
    }

    #[test]
    fn other_commander_lines_carry_the_name() {
        let mut c = Callout::new("danger", "2026-10-17T18:06:55Z", 3, true, "Under attack.".into());
        prefix_for_other(Some("Beta"), &mut c);
        assert_eq!(c.text, "Beta: Under attack.");
        let mut d = Callout::new("danger", "2026-10-17T18:06:55Z", 3, true, "Ship destroyed.".into());
        prefix_for_other(None, &mut d);
        assert_eq!(d.text, "Other commander: Ship destroyed.");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p edda other_commander`
Expected: FAIL to compile: `is_for_other_commander`, `prefix_for_other` not found.

- [ ] **Step 3: Implement the pure rules**

Add to `src-tauri/src/callouts.rs`:

```rust
/// What an UNSELECTED commander may say (maintainer, 2026-10-24: "danger
/// only"): the spoken danger callouts and a carrier jump being scheduled.
/// Everything else from that account stays in its store, unspoken.
pub fn is_for_other_commander(c: &Callout, event: Option<&Value>) -> bool {
    if !c.speak {
        return false;
    }
    c.kind == "danger" || event.and_then(|v| v.get("event")).and_then(Value::as_str) == Some("CarrierJumpRequest")
}

/// The other account's line says whose it is.
pub fn prefix_for_other(name: Option<&str>, c: &mut Callout) {
    let who = name.filter(|n| !n.trim().is_empty()).unwrap_or("Other commander");
    c.text = format!("{who}: {}", c.text);
}
```

- [ ] **Step 4: Rework the watcher loop**

In `src-tauri/src/watcher.rs`:

(a) Signature: `pub fn run(token: CancellationToken, app: AppHandle, registry: Arc<ed_store::registry::Registry>, journal_dir: &Path)`.

(b) Per-store state replaces the single `(watermark, cstate)`:
```rust
        use std::collections::HashMap;
        struct PerStore { watermark: Option<ed_store::session::Watermark>, cstate: CalloutState }
        let mut per: HashMap<String, PerStore> = HashMap::new();
```
seeded lazily: when a store key is first seen in the loop, `per.entry(fid).or_insert_with(|| { let g = store.lock()...; PerStore { watermark: ed_store::session::last_event_key(g.conn()).ok().flatten(), cstate: seed_state(g.conn()) } })`.

(c) The sync: `let synced = registry.sync_all(|_, _, _| {});` → `Ok(out)`. Then `let state = app.state::<AppState>(); state.adopt_new_commanders(&out.new_fids);` and `let selected = state.selected_fid(); let alerts_on = state.config.lock().unwrap_or_else(|e| e.into_inner()).other_commander_alerts;`.

(d) Emit `JOURNAL_CHANGED` when the selected store's `out.per_store[&selected]` has `events_inserted > 0 || snapshots_updated > 0`, or when `!out.new_fids.is_empty()` (the strip must learn about a new commander). Payload: the selected store's `events_inserted`.

(e) The prune block runs once over every store (`for (_, store) in registry.stores()`).

(f) The big per-event block becomes a loop `for (fid, store) in registry.stores()`: `let is_selected = fid == selected;` and `let st = per.entry(fid.clone()).or_insert_with(..)`. Inside, `conn` is that store's connection; `watermark`/`cstate` are `st.watermark`/`st.cstate`. Wrap the side-effect `match v.get("event")` block (the `MaterialCollected`/`Docked`/`CarrierJumpRequest`/`Undocked`/`Shutdown`/`FSDTarget`/`StartJump`/`FuelScoop`/`Scan`/`NavRoute`/`FSDJump` arms, watcher.rs:176-330) in `if is_selected { .. }`. Likewise the `from_status` call and anything that reads `Status.json` for speech run only when `is_selected`. The `SUPERCHARGE` emit runs only when `is_selected`.

(g) After `out` (the `Vec<Sourced>`) is collected for a store: if `!is_selected`, then
```rust
    if alerts_on {
        let name = ed_store::session::commander_name(conn).ok().flatten();
        out.retain(|(c, ev)| callouts::is_for_other_commander(c, ev.as_ref()));
        for (c, _) in out.iter_mut() { callouts::prefix_for_other(name.as_deref(), c); }
    } else {
        out.clear();
    }
```
then `announcer.deliver(out)` as today (the announcer is built from the state as before).

(h) `watermark` advancing and the `MAX_EVENTS_PER_PASS` cap stay per store.

In `src-tauri/src/lib.rs`, the spawn becomes:
```rust
                let registry = registry_handle.clone();
                let app = handle.clone();
                let journal_dir = journal_dir.clone();
                let _ = jobs.spawn_blocking(jobs::JOURNAL_WATCHER, move |token| {
                    watcher::run(token, app, registry, &journal_dir);
                });
```
and the temporary store handle from Task 6 (if any) is removed.

- [ ] **Step 5: Build, test, and run the dev app once**

Run: `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo test -p edda`
Expected: PASS, the 2 new tests included.

Run (smoke, from the worktree, with a copy of a real single-commander data dir or the maintainer's): `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits cargo tauri dev` is the maintainer's flight, not the executor's; the executor instead runs the replay harness again (Task 5's command) and confirms `purity ok` still prints, since Task 7 touched no store code.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/callouts.rs src-tauri/src/watcher.rs src-tauri/src/lib.rs
git commit -m "Multiple commanders: the watcher runs one pass per store -- the selected commander keeps today's whole pipeline (callouts, status, route and trade follow, carrier advance), every other commander's pass keeps only the spoken danger callouts and a carrier jump scheduled, prefixed with that commander's name, and only when Settings > Voice > alerts from other commanders is on (off by default; maintainer 2026-10-24: 'danger only'); journal-changed fires for the selected store or a new commander; tests RED first"
```

---

### Task 8: The frontend: the commander strip, the toggle, the list, the onboarding line

**Files:**
- Modify: `frontend/src/lib/api.js`
- Create: `frontend/src/lib/CommanderStrip.svelte`
- Modify: `frontend/src/App.svelte`
- Modify: `frontend/src/lib/VoiceSettings.svelte`
- Modify: `frontend/src/lib/SystemSettings.svelte`
- Modify: `frontend/src/lib/Onboarding.svelte`
- Test: `frontend/src/test/commanderstrip-renders.test.js`

**Interfaces:**
- Consumes: commands `list_commanders`, `select_commander` (fid), `other_commander_alerts`, `other_commander_alerts_set` (enabled) (Task 6); event `journal-changed`.
- Produces: `CommanderStrip` props `{ commanders: Array<{fid,name,ship,system,selected,wrote_s_ago,store_bytes}>, onSelect: (fid) => void }`.

- [ ] **Step 1: Write the failing render test**

Create `frontend/src/test/commanderstrip-renders.test.js`:

```js
// 2026-10-24: one EDDA, several of the player's own commanders, one HUD.
// The strip renders only when there is something to choose between.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import CommanderStrip from "../lib/CommanderStrip.svelte";

const two = [
  { fid: "F0000001", name: "Alpha", ship: "Mandalay", system: "Sol", selected: true, wrote_s_ago: 12, store_bytes: 1_200_000 },
  { fid: "F0000002", name: "Beta", ship: "Type-9 Heavy", system: "Sol", selected: false, wrote_s_ago: 4000, store_bytes: 300_000 },
];

describe("CommanderStrip", () => {
  it("renders one pill per commander with name, ship and system, the selected one marked", () => {
    const { body } = render(CommanderStrip, { props: { commanders: two, onSelect: () => {} } });
    expect(body).toContain("Alpha");
    expect(body).toContain("Beta");
    expect(body).toContain("Mandalay");
    expect(body).toContain("Type-9 Heavy");
    expect(body).toContain('aria-pressed="true"');
    expect(body).toContain('aria-pressed="false"');
    expect(body).toContain('class="live"');
    expect(body).not.toMatch(/\bsir\b|\bma'am\b/i);
  });

  it("renders nothing for a single commander", () => {
    const { body } = render(CommanderStrip, { props: { commanders: [two[0]], onSelect: () => {} } });
    expect(body.trim()).toBe("");
  });

  it("names a commander without a name by FID and skips the live dot when the journal is quiet", () => {
    const { body } = render(CommanderStrip, { props: { commanders: [{ ...two[0], name: null }, two[1]], onSelect: () => {} } });
    expect(body).toContain("F0000001");
    expect(body.match(/class="live"/g)?.length ?? 0).toBe(1);
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run commanderstrip`
Expected: FAIL: cannot resolve `../lib/CommanderStrip.svelte`.

- [ ] **Step 3: Implement the component**

Create `frontend/src/lib/CommanderStrip.svelte`:

```svelte
<script>
  // The player's own commanders, one pill each; click selects. Rendered
  // only with two or more: a single-account install looks as it always
  // did. Live dot: the journal wrote in the last minute. Nothing here is
  // another EDDA user — these are the accounts in this PC's journal folder.
  let { commanders = [], onSelect = () => {} } = $props();
  const label = (c) => c.name || c.fid;
  const detail = (c) => [c.ship, c.system].filter(Boolean).join(" · ");
</script>

{#if commanders.length > 1}
  <div class="commanders" role="group" aria-label="Commanders">
    {#each commanders as c (c.fid)}
      <button class="pill {c.selected ? 'ok' : ''}" aria-pressed={c.selected} onclick={() => onSelect(c.fid)} title={detail(c) || label(c)}>
        {#if c.wrote_s_ago != null && c.wrote_s_ago < 60}<span class="live" aria-label="in game now"></span>{/if}
        <strong>{label(c)}</strong>{#if detail(c)} <span class="muted">{detail(c)}</span>{/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .commanders { display: flex; flex-wrap: wrap; gap: 0.4rem; margin: 0.25rem 0 0.5rem; }
  .live { display: inline-block; width: 0.5rem; height: 0.5rem; border-radius: 50%; background: currentColor; margin-right: 0.35rem; vertical-align: middle; }
</style>
```

The `.pill`, `.pill.ok` and `.muted` classes are the app's existing ones (App.svelte's status line uses them); no new colour is introduced.

- [ ] **Step 4: Wire the API rows and the shell**

In `frontend/src/lib/api.js`, add under `// Status & inventory`:

```js
  listCommanders: ["list_commanders"],
  selectCommander: ["select_commander", ["fid"]],
  otherCommanderAlerts: ["other_commander_alerts"],
  otherCommanderAlertsSet: ["other_commander_alerts_set", ["enabled"]],
```

and add `listCommanders, selectCommander, otherCommanderAlerts, otherCommanderAlertsSet,` to the `export const { ... } = wrapped;` list (line ~302, the first line of names).

In `frontend/src/App.svelte`:
- import: `import CommanderStrip from "./lib/CommanderStrip.svelte";` and add `listCommanders, selectCommander` to the `api.js` import line.
- state: `let commanders = $state([]);` and
  ```js
  let commandersTimer = null;
  async function refreshCommanders() {
    try { commanders = await listCommanders(); } catch (e) { console.error("commanders unavailable:", e); }
  }
  async function pickCommander(fid) {
    try { commanders = await selectCommander(fid); } catch (e) { console.error("select commander failed:", e); }
  }
  ```
- in `onMount`, after the `getStatus` probe: `await refreshCommanders();` and extend the existing `onJournalChanged` listener (or add one if none exists in App.svelte) to throttle a refresh: `listeners.add(onJournalChanged(() => { clearTimeout(commandersTimer); commandersTimer = setTimeout(refreshCommanders, 2000); }));` (import `onJournalChanged` from api.js if not already).
- markup: inside `<header>`, after the `<h1>` line and before `<nav>`: `<CommanderStrip {commanders} onSelect={pickCommander} />`.

In `frontend/src/lib/VoiceSettings.svelte`, in the Callouts section after the `{#each calloutKinds ...}` row, add:

```svelte
  <p class="muted small" style="margin:0.5rem 0 0">
    <label><input type="checkbox" checked={otherAlerts} onchange={async (e) => { otherAlerts = e.target.checked; await otherCommanderAlertsSet(otherAlerts); }} />
    Alerts from other commanders</label> — when more than one of your accounts is in game, the unselected one speaks only danger: under attack, interdicted, overheating, destroyed, carrier jump scheduled. Off by default.
  </p>
```

with `let otherAlerts = $state(false);`, `otherCommanderAlerts().then((v) => (otherAlerts = v)).catch(() => {});` in its `onMount`, and `otherCommanderAlerts, otherCommanderAlertsSet` added to its api.js import.

In `frontend/src/lib/SystemSettings.svelte`, in the Database section (find the `dbStats` display), add a read-only list:

```svelte
  {#if commanders.length > 1}
    <dt>Commanders</dt>
    <dd>{#each commanders as c (c.fid)}<div><strong>{c.name || c.fid}</strong> <span class="muted">{(c.store_bytes / 1e6).toFixed(1)} MB{c.selected ? " · selected" : ""}</span></div>{/each}
      <p class="muted small" style="margin:0.3rem 0 0">Each account in this PC's journal folder has its own database; the selector above the tabs picks which one EDDA shows, speaks and overlays.</p></dd>
  {/if}
```

with `let commanders = $state([]);`, `listCommanders().then((v) => (commanders = v)).catch(() => {});` in `onMount`, and `listCommanders` imported.

In `frontend/src/lib/Onboarding.svelte`, on the step that reports the journal folder (search `journal` near step 1-2), add after the folder line:

```svelte
  {#if commanders.length > 1}<p class="muted small">Two or more commanders share this journal folder. EDDA keeps each one apart and follows <strong>{commanders.find((c) => c.selected)?.name ?? "the first"}</strong>; switch with the pills above the tabs.</p>{/if}
```

with the same `commanders` state and `listCommanders` import.

- [ ] **Step 5: Run the gates**

Run: `cd frontend && npx vitest run && npx vite build`
Expected: all tests PASS (the contract test now sees the four commands in both `api.js` and `generate_handler!`); the build has no a11y warning (the button has text content and a title; the live dot has `aria-label`).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/api.js frontend/src/lib/CommanderStrip.svelte frontend/src/App.svelte frontend/src/lib/VoiceSettings.svelte frontend/src/lib/SystemSettings.svelte frontend/src/lib/Onboarding.svelte frontend/src/test/commanderstrip-renders.test.js
git commit -m "Multiple commanders: the commander strip above the tabs (one pill per account in the journal folder, rendered only for two or more, live dot for a journal that wrote in the last minute, click selects and every panel re-derives), the alerts-from-other-commanders toggle under Voice (off by default), the per-commander database list under Settings, one onboarding line; SSR render test RED first, vitest and vite build green"
```

---

### Task 9: The record

**Files:**
- Modify: `docs/ROADMAP.md`
- Modify: `docs/superpowers/specs/2026-10-24-multi-commander-design.md` (status line)

- [ ] **Step 1: Roadmap entry**

Under the current month's open-work section of `docs/ROADMAP.md` (the paragraph that starts the October record), add:

```
- **Multiple commanders (2026-10-24, the maintainer: "multiple accounts /
  multiboxing"; branch `multi-commander`):** one store per FID
  (`edda.<FID>.sqlite3`), journal files attributed by their first Commander
  line, companion files by content (Status.json by Balance), a manual
  commander strip, one HUD and voice on the selected commander, danger-only
  alerts from the others behind an off-by-default toggle. Measured first:
  the shipped store blended a two-account folder 263 commander flips in 90
  days (`docs/benches/2026-10-24-two-account-folder-baseline.csv`); the
  registry replay: `docs/benches/2026-10-24-multibox-replay.csv`. Spec:
  `docs/superpowers/specs/2026-10-24-multi-commander-design.md`. OPEN:
  selection following the focused game window (process start time against
  the Fileheader timestamp, unverified); a both-at-once wing view; separate
  journal folders on one machine; the release waits on a flight on a real
  two-instance machine (the donor is the natural tester). Release notes are
  written at the cut, not here (the newest section must equal the version).
```

- [ ] **Step 2: Spec status**

Change the spec's second line from `Status: DRAFT for the maintainer's review. Nothing below is built;` to `Status: APPROVED 2026-10-24; implemented on branch multi-commander (plan docs/superpowers/plans/2026-10-24-multi-commander.md).` keeping the rest of the sentence about the baseline.

- [ ] **Step 3: Commit**

```bash
git add docs/ROADMAP.md docs/superpowers/specs/2026-10-24-multi-commander-design.md
git commit -m "Multiple commanders: the roadmap entry and the spec's status -- what shipped on the branch, the two bench records, what stays OPEN (focus-follow selection, wing view, separate folders), and that the release waits on a two-instance flight"
```

---

## Self-review notes

- Spec coverage: §1 store and ingest → Tasks 1-4; companions by content → Task 3 + Task 4 dispatch; menu-only files → Tasks 1, 4; refusal of a second FID → Task 2; §2 registry in the app, selection, HUD/voice binding, danger set, Frontier slot, follow unchanged, EDDN per store (each store keeps its own watermark table; no code change), telemetry unchanged → Tasks 6-7; §3 strip, commands, toggle, list, onboarding, tests → Task 8; §4 baseline (done before this plan), harness → Task 5, sync-cost pin → Task 5's quiet passes, existing users → Task 4's legacy tests, flight → Task 9 names it, OPEN items → Task 9.
- Open question in the spec (snapshots table shared vs per store): resolved per store with `{}` as unknown, which keeps every reader unchanged (Task 2/4).
- Type consistency: `Owner::{Fid, Pending, MenuOnly}` (Task 1) is what Task 4 matches; `ingest_files(conn, dir, names, expect_fid, progress)` (Task 2) is what `Store::ingest_owned` calls (Task 4); `Score`/`choose` (Task 3) are what `sync_all` uses (Task 4); `Registry::{open, db_path_for, keys, fids, store, stores, last_writer, sync_all, base}` (Task 4) are what Tasks 5-7 call; `CommanderRow` fields (Task 6) are what the strip reads (Task 8).
- Review Focus items each have a named test in Tasks 1, 4, 5, 7.
