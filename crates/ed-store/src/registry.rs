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

/// A file with no Commander line that nobody has written to for this long
/// is a launcher opened and killed at the menu, not a session still
/// loading: 87 of the donor's 164 launcher-only files never wrote
/// `Shutdown` (replay, 2026-10-24) and would otherwise stay pending, and
/// be re-read, forever. A file the game is still writing has a fresh mtime.
const STALE_PENDING: std::time::Duration = std::time::Duration::from_secs(3600);

fn is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|m| m.elapsed().unwrap_or_default() > STALE_PENDING)
        .unwrap_or(false)
}

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
    /// Files called MenuOnly for being stale, with the (mtime, size) that
    /// earned it: if either moves, the file is classified again.
    stale: HashMap<String, (std::time::SystemTime, u64)>,
    /// Each companion file's mtime when it was last dispatched; an
    /// unchanged file is not read, parsed or scored again.
    companion_mtimes: HashMap<String, Option<i64>>,
    /// The commander synced first (the selected one): on a first sync a
    /// large other store must not keep this one's panels empty.
    first: Option<String>,
}

fn file_key(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.modified().ok()?, md.len()))
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
            inner: Mutex::new(Inner { stores, owners: HashMap::new(), stale: HashMap::new(), companion_mtimes: HashMap::new(), first: None }),
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

    /// Sync this commander's store before the others (the app calls it on
    /// every selection).
    pub fn set_first(&self, fid: &str) {
        self.inner().first = Some(fid.to_string());
    }

    pub fn first(&self) -> Option<String> {
        self.inner().first.clone()
    }

    /// The stores in the order a sync ingests and derives them: the
    /// `first` commander, then the rest by FID.
    pub fn sync_order(&self) -> Vec<(String, Arc<Mutex<Store>>)> {
        let first = self.first();
        let mut all = self.stores();
        all.sort_by_key(|(fid, _)| Some(fid) != first.as_ref());
        all
    }

    /// The commander whose journal file was modified most recently. Works
    /// before the first sync (the launch selection needs it): the folder
    /// is listed by mtime and the newest files are read for their owner
    /// until one names a commander.
    pub fn last_writer(&self) -> Option<String> {
        let names = ingest::journal_file_names(&self.journal_dir).ok()?;
        let mut by_mtime: Vec<(std::time::SystemTime, String)> = names
            .into_iter()
            .filter_map(|n| std::fs::metadata(self.journal_dir.join(&n)).and_then(|m| m.modified()).ok().map(|t| (t, n)))
            .collect();
        by_mtime.sort_by(|a, b| b.0.cmp(&a.0));
        let inner = self.inner();
        for (_, name) in by_mtime {
            let known = inner.owners.get(&name).cloned();
            let o = match known {
                Some(o) => o,
                None => owner(&self.journal_dir.join(&name)).unwrap_or(Owner::Pending),
            };
            if let Owner::Fid(fid) = o {
                return Some(fid);
            }
        }
        None
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
                let path = self.journal_dir.join(name);
                let known = inner.owners.get(name).cloned();
                // A stale MenuOnly whose file moved since is classified again:
                // the player sat at the menu for an hour, then logged in.
                let moved = inner.stale.get(name).is_some_and(|k| file_key(&path).as_ref() != Some(k));
                let o = match known {
                    Some(Owner::Fid(f)) => Owner::Fid(f),
                    Some(Owner::MenuOnly) if !moved => Owner::MenuOnly,
                    _ => {
                        let mut o = owner(&path).unwrap_or(Owner::Pending);
                        if o == Owner::Pending && is_stale(&path) {
                            o = Owner::MenuOnly;
                            if let Some(k) = file_key(&path) {
                                inner.stale.insert(name.clone(), k);
                            }
                        } else {
                            inner.stale.remove(name);
                        }
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

        let stores = self.sync_order();
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

        // 3. Companions by content. An unchanged file (same mtime as when
        // it was last dispatched) is not read: nine files against every
        // store on every idle pass is the cost the 2026-10-03 fix removed.
        let seen = self.inner().companion_mtimes.clone();
        for name in ingest::COMPANION_FILES {
            let path = self.journal_dir.join(name);
            let Ok(md) = std::fs::metadata(&path) else { continue };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            if seen.get(*name) == Some(&mtime) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else { continue };
            let raw = raw.trim().trim_start_matches('\u{feff}').to_string();
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
            let ts = v.get("timestamp").and_then(|x| x.as_str());
            let mut scores = Vec::new();
            if stores.len() == 1 {
                // One commander takes everything: nothing to score.
                scores.push((stores[0].0.clone(), crate::companions::Score::Unknown));
            } else {
                for (fid, store) in &stores {
                    let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                    scores.push((fid.clone(), crate::companions::score(guard.conn(), name, &v)?));
                }
            }
            let chosen = crate::companions::resolve_tie(name, &v, crate::companions::choose(&scores));
            self.inner().companion_mtimes.insert((*name).to_string(), mtime);
            for (fid, store) in &stores {
                let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                // Unchanged since the stored copy: nothing to do (same mtime rule as ingest_companions).
                let stored: Option<Option<i64>> = guard
                    .conn()
                    .query_row("SELECT mtime FROM snapshots WHERE name = ?1", [name], |r| r.get(0))
                    .ok();
                if chosen.iter().any(|c| c == fid) {
                    if stored != Some(mtime) {
                        ingest::write_snapshot(guard.conn(), name, ts, mtime, &raw)?;
                        if let Some(s) = ingested.get_mut(fid) {
                            s.snapshots_updated += 1;
                        }
                    }
                } else if chosen.is_empty() {
                    if stored != Some(None) {
                        ingest::mark_unknown(guard.conn(), name)?;
                        if let Some(s) = ingested.get_mut(fid) {
                            s.snapshots_updated += 1;
                        }
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
                conn.execute("ATTACH DATABASE ?1 AS legacy", [blended.to_string_lossy().replace('\\', "/")])?;
                conn.execute_batch("INSERT INTO user_marks SELECT * FROM legacy.user_marks; DETACH DATABASE legacy;")?;
            }
            tracing::warn!(commanders = many.len(), kept = %blended.display(), "legacy store held two commanders; split, bookmarks copied to each, the journal re-ingests");
            Ok(())
        }
    }
}

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
    #[test]
    fn the_selected_commander_is_synced_first() {
        // 2026-10-24 dev-app run: the first sync derived the big store (1M
        // events, ~10 min in debug) before the small one, so the selected
        // small commander's panels sat empty behind the other account.
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}
{LOAD_A}
")).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}
{LOAD_B}
")).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        reg.sync_all(|_, _, _| {}).unwrap();
        let order = |r: &Registry| r.sync_order().into_iter().map(|(f, _)| f).collect::<Vec<_>>();
        assert_eq!(order(&reg), vec!["F0000001".to_string(), "F0000002".to_string()]);
        reg.set_first("F0000002");
        assert_eq!(order(&reg), vec!["F0000002".to_string(), "F0000001".to_string()]);
        assert_eq!(reg.first().as_deref(), Some("F0000002"));
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
        // Before any sync (launch): the app picks its commander from this.
        assert_eq!(reg.last_writer(), Some("F0000002".to_string()), "the last writer is known before the first sync");
        reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(reg.last_writer(), Some("F0000002".to_string()));
        let (_d2, journal2, base2) = folder();
        let empty = Registry::open(&base2, &journal2).unwrap();
        assert_eq!(empty.last_writer(), None);
    }

    #[test]
    fn a_stale_file_without_a_commander_is_menu_only_not_pending_forever() {
        // 87 of the donor's 164 launcher-only files never wrote Shutdown
        // (replay, 2026-10-24): the game was killed at the menu. A file
        // nobody has written to for an hour that never named a commander
        // is dead, not pending; a fresh one may still get its Commander line.
        let (_d, journal, base) = folder();
        let stale = journal.join("Journal.2026-10-17T190000.01.log");
        std::fs::write(&stale, concat!(r#"{"timestamp":"2026-10-17T19:00:00Z","event":"Fileheader","part":1}"#, "\n")).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
        std::fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T191000.01.log"), concat!(r#"{"timestamp":"2026-10-17T19:10:00Z","event":"Fileheader","part":1}"#, "\n")).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        let out = reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!((out.menu_only_files, out.pending_files), (1, 1), "the stale file is menu-only, the fresh one pending");
        // The player sat at the menu for an hour, then logged in: the file
        // grew and its mtime moved, so it is read again, not ignored for
        // the rest of the session.
        std::fs::write(&stale, format!("{{\"timestamp\":\"2026-10-17T19:00:00Z\",\"event\":\"Fileheader\",\"part\":1}}\n{CMDR_A}\n{LOAD_A}\n")).unwrap();
        let out = reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(out.new_fids, vec!["F0000001".to_string()], "a stale file that grows is classified again");
        assert_eq!(count(&reg.store("F0000001").unwrap(), "SELECT COUNT(*) FROM events"), 3);
    }

    #[test]
    fn a_commander_store_refuses_to_sync_on_its_own() {
        // Store::sync ingests every file in the folder with no FID check:
        // on a commander's store that would blend the other account in
        // (the reviewer's finding, 2026-10-24: Settings > Sync now did it).
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n")).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}\n{LOAD_B}\n")).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        reg.sync_all(|_, _, _| {}).unwrap();
        let a = reg.store("F0000001").unwrap();
        assert!(a.lock().unwrap().sync().is_err(), "a commander's store syncs only through the registry");
        assert_eq!(count(&a, "SELECT COUNT(*) FROM events WHERE json_extract(raw,'$.FID') = 'F0000002'"), 0);
        assert_eq!(count(&a, "SELECT COUNT(*) FROM events"), 2);
    }

    #[test]
    fn a_quiet_pass_dispatches_no_companion() {
        // Idle syncs must not read, parse and score nine files against
        // every store (2026-10-03: the idle re-read cost 153 MB per sync).
        let (_d, journal, base) = folder();
        std::fs::write(journal.join("Journal.2026-10-17T180648.01.log"), format!("{CMDR_A}\n{LOAD_A}\n{CARGO_A}\n")).unwrap();
        std::fs::write(journal.join("Journal.2026-10-17T180748.01.log"), format!("{CMDR_B}\n{LOAD_B}\n{CARGO_B}\n")).unwrap();
        std::fs::write(journal.join("Status.json"), r#"{"timestamp":"2026-10-17T18:09:00Z","event":"Status","Balance":9000012345,"Fuel":{"FuelMain":30.0}}"#).unwrap();
        let reg = Registry::open(&base, &journal).unwrap();
        let first = reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(first.companions.len(), 1, "the first pass dispatches the file");
        let quiet = reg.sync_all(|_, _, _| {}).unwrap();
        assert!(quiet.companions.is_empty(), "an unchanged companion is not dispatched again: {:?}", quiet.companions);
        // The file is written again (new mtime): dispatched once more.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(journal.join("Status.json"), r#"{"timestamp":"2026-10-17T18:10:00Z","event":"Status","Balance":9000012345,"Fuel":{"FuelMain":29.0}}"#).unwrap();
        let again = reg.sync_all(|_, _, _| {}).unwrap();
        assert_eq!(again.companions.len(), 1);
    }
}
