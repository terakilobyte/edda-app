use crate::callouts::Callout;
use crate::voice::VoiceHandle;
use ed_engineering::Catalog as EngineeringCatalog;
use ed_store::Store;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// How many recent callouts the overlay can ask for on startup.
const CALLOUT_HISTORY: usize = 50;

/// App-managed settings, persisted as `.data/config.json`.
///
/// The API key lives here so nobody has to fight Windows environment
/// variables; `ANTHROPIC_API_KEY` in the environment still overrides it.
/// Plain JSON in the gitignored data directory -- not encrypted, and the
/// Settings panel says so.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub anthropic_model: Option<String>,
    /// "anthropic" (default) or "openai" (any OpenAI-compatible endpoint:
    /// Mistral, Ollama, LM Studio, OpenRouter, OpenAI...).
    #[serde(default)]
    pub ai_provider: Option<String>,
    /// Base URL of the OpenAI-compatible endpoint, e.g. https://api.mistral.ai/v1
    #[serde(default)]
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub openai_model: Option<String>,
    #[serde(default)]
    pub persona: Option<String>,
    /// Let the ship computer use Anthropic's server-side web search and
    /// fetch for game-mechanics questions. `None` means the default (on).
    #[serde(default)]
    pub research: Option<bool>,
    /// Steps of the "target next system" macro; `None` = the default.
    #[serde(default)]
    pub target_macro: Option<Vec<crate::follow::MacroStep>>,
    /// Where the galaxy map's search box, first result, target button, and
    /// plot-route button are on screen, taught by clicking them once.
    #[serde(default)]
    pub map_points: Option<crate::follow::MapPoints>,
    /// A key or joystick button the commander mapped as "next system"
    /// for app routes -- best the same one the game uses, so both agree.
    #[serde(default)]
    pub target_trigger: Option<crate::listen::PttSource>,
    /// Drive the galaxy map with the macro for "target next" on an app
    /// route. Off by default: the clipboard is instant and never misfires.
    #[serde(default)]
    pub target_macro_enabled: bool,
    #[serde(default = "default_game_route_ly")]
    pub game_route_max_ly: u32,
    /// Voice input: wake word, push-to-talk key, enabled.
    #[serde(default)]
    pub listen: Option<crate::listen::ListenConfig>,
    /// Speech through a server (Kokoro etc.) instead of the built-in voice.
    #[serde(default)]
    pub voice_server: Option<ed_voice::ServerConfig>,
    #[serde(default)]
    pub voice_server_enabled: bool,
    /// The version whose release notes the commander has seen; a
    /// mismatch with the running version triggers the what's-new splash
    /// once after an update.
    #[serde(default)]
    pub notes_seen_version: Option<String>,
    /// Signal sources to announce when they appear on sensors (ids from callouts::SIGNALS).
    #[serde(default)]
    pub signal_watch: Option<Vec<String>>,
    /// Callout kinds switched off (neither spoken nor shown).
    #[serde(default)]
    pub callouts_off: Vec<String>,
    /// Public EDDA community-data API. `EDDA_API_URL` overrides this for
    /// development and self-hosted deployments.
    #[serde(default)]
    pub community_api_url: Option<String>,
    /// Dev-build toggle (thin-client arc, maintainer 2026-09-05): route ALL
    /// API traffic to the local WSL dev server instead of the community
    /// server. Release builds ignore this entirely.
    #[serde(default)]
    pub dev_api_local: Option<bool>,
    /// This install's random id, sent as `X-EDDA-Install` so the API's
    /// budgets are per install rather than per address (API-only spec,
    /// "Per-install keys"; 2026-09-09: a c=4 ladder showed per-IP burst
    /// caps land on everyone behind one NAT). 32 hex from OS randomness,
    /// minted once and kept; it names an install to the limiter and
    /// nothing else - no account, no identity, nothing that locates a
    /// commander.
    #[serde(default)]
    pub install_id: Option<String>,
    /// Check for a new EDDA release on its own (on by default); off
    /// means the Settings button only. Once also governed the data
    /// auto-update; the local data went with B.4 (2026-09-09).
    #[serde(default)]
    pub auto_update: Option<bool>,
    /// Anonymous usage telemetry (maintainer consent ruling 2026-09-05):
    /// opt-OUT — `None` means enabled; `Some(false)` is the unchecked box.
    #[serde(default)]
    pub send_telemetry: Option<bool>,
}

fn default_game_route_ly() -> u32 { 1_000 }

impl AppConfig {
    pub fn research_enabled(&self) -> bool {
        self.research.unwrap_or(true)
    }

    pub fn path(data_dir: &std::path::Path) -> PathBuf {
        data_dir.join("config.json")
    }

    pub fn load(data_dir: &std::path::Path) -> Self {
        let mut cfg: AppConfig = std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if cfg.game_route_max_ly == 0 { cfg.game_route_max_ly = default_game_route_ly(); }
        if !cfg.install_id.as_deref().is_some_and(is_install_id) {
            cfg.install_id = Some(mint_install_id());
            if let Err(error) = cfg.save(data_dir) {
                tracing::warn!(%error, "could not persist the install id; a new one is minted next start");
            }
        }
        // Keys used to live in this file. Move any that still does into the
        // credential store and scrub it from disk.
        if let Some(k) = cfg.anthropic_api_key.take() {
            if !k.trim().is_empty() && secrets::set_api_key(&k).is_ok() {
                tracing::info!("migrated API key from config.json to the credential store");
            }
            let _ = cfg.save(data_dir);
        }
        cfg
    }

    pub fn save(&self, data_dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        std::fs::write(
            Self::path(data_dir),
            serde_json::to_string_pretty(self).unwrap_or_default(),
        )
    }
}

/// Shared app state.
///
/// The store replaces the old "re-read six journal files on every request"
/// approach: history is assembled once at startup and then tailed, so a
/// command is a table read rather than a parse. `rusqlite::Connection` is
/// `Send` but not `Sync`, hence the mutex; it is held in an `Arc` so the
/// background EDDN task can write through the same connection rather than
/// opening a second writer against the same file.
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub engineering: EngineeringCatalog,
    /// The one async HTTP client, `EDDA/<version>` user agent. Every
    /// async HTTP site borrows it; per-request timeouts go on the request.
    pub http: reqwest::Client,
    /// Its blocking twin for the streaming importers and the sidecar
    /// installers, which read bodies on the blocking pool.
    pub http_blocking: reqwest::blocking::Client,
    pub voice: Arc<VoiceHandle>,
    pub callouts: Arc<Mutex<VecDeque<Callout>>>,
    pub data_dir: PathBuf,
    /// Whether the overlay currently accepts mouse input (false = click-through).
    pub overlay_interactive: AtomicBool,
    /// Was the HUD visible when the main window went to the tray? Hiding
    /// to the tray takes the HUD with it, and opening from the tray puts
    /// back what the commander had -- not blindly a visible HUD, or
    /// Ctrl+Shift+H would be undone by every trip through the tray.
    pub overlay_visible_before_tray: AtomicBool,
    pub config: Arc<Mutex<AppConfig>>,
    pub db_path: PathBuf,
    /// Galaxy index caches; also managed separately for the routing commands.
    pub routing: Arc<crate::routing::RoutingState>,
    /// Where events go. The live adapter is installed once the app is up;
    /// code that only has the state (the AI tool executor) emits here.
    pub events: Arc<crate::events::EventBus>,
    /// Every background task, named, cancellable, joined on exit.
    pub jobs: Arc<crate::jobs::Supervisor>,
    /// Is the game process running? Maintained by the game-poll job.
    pub game_running: Arc<AtomicBool>,
    /// Voice-input state machine: phase, loop generation, push-to-talk,
    /// setup, the loaded Parakeet, the registered hotkey. One value, one
    /// mutex, instead of eight process globals.
    pub listen: Mutex<crate::listen::ListenState>,
    /// Ship computer conversation: alternating user/assistant turns (final
    /// text only, never the tool traffic), sent with every question so it can
    /// refer back. Cleared by `ai_reset`.
    pub chat: Mutex<Vec<serde_json::Value>>,
    /// Idle read-only connections, galaxy already attached. Opening one
    /// costs a few ms (and the 31 GB attach); reusing costs nothing.
    pub readers: Mutex<Vec<rusqlite::Connection>>,
    /// Times `with_read` could not open a private connection and ran on the
    /// shared writer instead. Shown by `db_stats`; a non-zero count means
    /// readers are queueing behind the writer again.
    pub read_fallbacks: std::sync::atomic::AtomicU64,
    /// External runtimes launched by EDDA. Platform-specific process-tree
    /// ownership guarantees they cannot survive the app.
    pub helpers: crate::helpers::HelperManager,
}

/// How the app introduces itself to every HTTP service.
pub const USER_AGENT: &str = concat!("EDDA/", env!("CARGO_PKG_VERSION"), " (Elite Dangerous Desktop Aid)");

/// Idle private read connections kept for reuse.
pub const READ_POOL: usize = 8;

impl AppState {
    pub fn new(store: Store, data_dir: PathBuf, db_path: PathBuf) -> Self {
        let voice = Arc::new(VoiceHandle::spawn(&data_dir, USER_AGENT));
        let config = Arc::new(Mutex::new(AppConfig::load(&data_dir)));
        let install_header = {
            let id = config.lock().unwrap_or_else(|e| e.into_inner()).install_id.clone().unwrap_or_default();
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&id) {
                headers.insert("x-edda-install", value);
            }
            headers
        };
        let runtime = tauri::async_runtime::handle().inner().clone();
        let jobs = Arc::new(crate::jobs::Supervisor::new(runtime));
        AppState {
            config,
            db_path,
            routing: { let r = Arc::new(crate::routing::RoutingState::new()); r.attach_jobs(jobs.clone()); r },
            events: Arc::new(crate::events::EventBus::default()),
            jobs: jobs.clone(),
            game_running: Arc::new(AtomicBool::new(false)),
            listen: Mutex::new(crate::listen::ListenState::default()),
            chat: Mutex::new(Vec::new()),
            readers: Mutex::new(Vec::new()),
            read_fallbacks: std::sync::atomic::AtomicU64::new(0),
            helpers: crate::helpers::HelperManager::new(),
            store: Arc::new(Mutex::new(store)),
            engineering: EngineeringCatalog::load(),
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .default_headers(install_header.clone())
                .build()
                .expect("http client"),
            http_blocking: reqwest::blocking::Client::builder()
                .user_agent(USER_AGENT)
                .default_headers(install_header)
                .build()
                .expect("blocking http client"),
            voice,
            callouts: Arc::new(Mutex::new(VecDeque::with_capacity(CALLOUT_HISTORY))),
            data_dir,
            overlay_interactive: AtomicBool::new(false),
            overlay_visible_before_tray: AtomicBool::new(true),
        }
    }

    /// The pieces callout delivery needs, cloneable into a background job.
    pub fn announcer(&self) -> crate::watcher::Announcer {
        crate::watcher::Announcer {
            config: self.config.clone(),
            callouts: self.callouts.clone(),
            voice: self.voice.clone(),
            events: self.events.clone(),
        }
    }

    /// A private read-only connection for long queries.
    ///

    /// in WAL mode serves any number of readers alongside one writer, so
    /// heavy reads open their own connection and never touch the lock.
    pub fn read_conn(&self) -> Result<rusqlite::Connection, String> {
        use rusqlite::OpenFlags;
        let conn = rusqlite::Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
        ed_store::schema::attach_galaxy(&conn, Some(&ed_store::schema::galaxy_path(&self.db_path)))
            .map_err(|e| e.to_string())?;
        Ok(conn)
    }

    /// Run a read-only query on a private connection, so readers never
    /// queue behind the writer or behind each other. Falls back to the
    /// shared connection only if a new one cannot be opened.
    pub fn with_read<T>(&self, f: impl FnOnce(&Reader<'_>) -> T) -> T {
        let pooled = self.readers.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let conn = match pooled {
            Some(c) => Ok(c),
            None => self.read_conn(),
        };
        match conn {
            Ok(conn) => {
                let out = f(&Reader { conn: &conn });
                let mut pool = self.readers.lock().unwrap_or_else(|e| e.into_inner());
                if pool.len() < READ_POOL {
                    pool.push(conn);
                }
                out
            }
            Err(e) => {
                // Loud: every read now queues behind the writer, which is
                // the exact regression the pool exists to prevent.
                let n = self.read_fallbacks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                tracing::error!(error = %e, fallbacks = n, "read connection failed; using the shared writer");
                let guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
                f(&Reader { conn: guard.conn() })
            }
        }
    }

    /// How often `with_read` had to fall back to the shared writer.
    pub fn read_fallbacks(&self) -> u64 {
        self.read_fallbacks.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Run `f` against the store.
    ///
    /// A poisoned mutex means another thread panicked mid-write. The
    /// connection itself is still usable and SQLite's own transactions
    /// guarantee consistency, so recovering beats taking the whole app down
    /// over one failed command.
    pub fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> T {
        let guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }
}

/// A read-only view handed to `with_read` closures. Same `conn()` shape as
/// `Store`, so a command switches between them by changing one call.
pub struct Reader<'a> {
    conn: &'a rusqlite::Connection,
}

impl Reader<'_> {
    pub fn conn(&self) -> &rusqlite::Connection {
        self.conn
    }
}

/// Secrets live in the OS credential store -- Windows Credential Manager
/// here -- never in a file the app writes. `keyring` handles the platform.
pub mod secrets {
    const SERVICE: &str = "edda";
    const API_KEY: &str = "anthropic_api_key";
    /// Key for the OpenAI-compatible provider (one slot; switching providers re-enters it).
    pub const OPENAI_KEY: &str = "openai_api_key";

    fn entry() -> Result<keyring::Entry, String> {
        entry_for(API_KEY)
    }

    fn entry_for(name: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, name).map_err(|e| e.to_string())
    }

    pub fn get_key(name: &str) -> Option<String> {
        entry_for(name)
            .ok()?
            .get_password()
            .ok()
            .filter(|k| !k.trim().is_empty())
    }

    pub fn set_key(name: &str, key: &str) -> Result<(), String> {
        entry_for(name)?
            .set_password(key.trim())
            .map_err(|e| e.to_string())
    }

    pub fn clear_key(name: &str) -> Result<(), String> {
        match entry_for(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn get_api_key() -> Option<String> {
        entry()
            .ok()?
            .get_password()
            .ok()
            .filter(|k| !k.trim().is_empty())
    }

    pub fn set_api_key(key: &str) -> Result<(), String> {
        entry()?.set_password(key.trim()).map_err(|e| e.to_string())
    }

    pub fn clear_api_key() -> Result<(), String> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn secrets_use_a_native_credential_store() {
            // Without a per-target keyring feature the crate compiles in its
            // in-memory mock, which forgets every API key at exit.
            use keyring::credential::CredentialPersistence;
            let p = keyring::default::default_credential_builder().persistence();
            assert!(!matches!(p, CredentialPersistence::EntryOnly), "keyring is using its mock store");
        }
    }
}

/// Remember a callout for late-joining windows. Shared by the watcher and
/// the startup greeting, which is why it takes the buffer rather than state.
pub fn remember(buf: &Mutex<VecDeque<Callout>>, c: &Callout) {
    let mut q = buf.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() >= CALLOUT_HISTORY {
        q.pop_front();
    }
    q.push_back(c.clone());
}

#[cfg(test)]
pub(crate) fn test_state(dir: &std::path::Path) -> AppState {
    let db = dir.join("edda.sqlite3");
    let journal = dir.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let store = Store::open(&db, &journal).unwrap();
    let state = AppState::new(store, dir.to_path_buf(), db);
    // A test state discovers SAPI (no models in a tempdir), so a delivered
    // callout would be spoken aloud in Windows voice by `cargo test`. Mute
    // it -- unless the test-voice feature asks to actually hear output.
    #[cfg(not(feature = "test-voice"))]
    state.voice.set_muted(true);
    state
}

#[cfg(test)]
mod test_state_tests {
    use super::*;

    /// Unit tests deliver real callouts through the announcer; the voice
    /// they reach must be muted, or `cargo test` speaks "Game detected.
    /// Ship computer online and listening." aloud in Windows voice --
    /// which haunted an evening of installer testing as a ghost voice.
    #[test]
    #[cfg(not(feature = "test-voice"))]
    fn a_test_state_never_speaks_aloud() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        assert!(state.voice.is_muted(), "test_state must mute the voice");
    }
}

#[cfg(test)]
mod read_pool_tests {
    use super::*;

    fn attached(conn: &rusqlite::Connection) -> Vec<String> {
        let mut st = conn.prepare("PRAGMA database_list").unwrap();
        st.query_map([], |r| r.get::<_, String>(1)).unwrap().flatten().collect()
    }

    #[test]
    fn with_read_uses_a_private_connection_with_galaxy_attached() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let writer = state.with_store(|s| s.conn() as *const rusqlite::Connection);
        let (dbs, reader) = state.with_read(|r| (attached(r.conn()), r.conn() as *const rusqlite::Connection));
        assert!(dbs.iter().any(|d| d == "galaxy"), "galaxy not attached: {dbs:?}");
        assert_ne!(reader, writer, "a read must not run on the shared writer");
        assert_eq!(state.read_fallbacks(), 0);
    }

    #[test]
    fn with_read_pool_caps_idle_connections() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        fn nest(state: &AppState, depth: usize) {
            if depth == 0 {
                return;
            }
            state.with_read(|_| nest(state, depth - 1));
        }
        nest(&state, READ_POOL + 4);
        let idle = state.readers.lock().unwrap().len();
        assert_eq!(idle, READ_POOL, "pool must cap at {READ_POOL}, held {idle}");
        assert_eq!(state.read_fallbacks(), 0);
    }

    #[test]
    fn with_read_falls_back_to_the_writer_loudly_and_counts_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("edda.sqlite3");
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let store = Store::open(&db, &journal).unwrap();
        // A db_path nobody can open read-only: every private read fails.
        let missing = dir.path().join("nope").join("edda.sqlite3");
        let state = AppState::new(store, dir.path().to_path_buf(), missing);
        let writer = state.with_store(|s| s.conn() as *const rusqlite::Connection);
        let reader = state.with_read(|r| r.conn() as *const rusqlite::Connection);
        assert_eq!(reader, writer, "must still answer, on the shared writer");
        assert_eq!(state.read_fallbacks(), 1, "the fallback is counted");
        state.with_read(|_| ());
        assert_eq!(state.read_fallbacks(), 2);
    }
}

/// 32 lowercase hex characters, as the API's limiter key expects.
pub fn is_install_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn mint_install_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("os randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod install_id_tests {
    use super::*;

    /// A fresh data dir gets one id, persisted, and keeps it; a mangled
    /// one is replaced rather than sent.
    #[test]
    fn an_install_id_is_minted_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = AppConfig::load(dir.path()).install_id.expect("minted");
        assert!(is_install_id(&first), "{first}");
        let again = AppConfig::load(dir.path()).install_id.unwrap();
        assert_eq!(first, again, "persisted, not re-minted");
        let mut cfg = AppConfig::load(dir.path());
        cfg.install_id = Some("not-an-id".into());
        cfg.save(dir.path()).unwrap();
        let replaced = AppConfig::load(dir.path()).install_id.unwrap();
        assert!(is_install_id(&replaced) && replaced != first);
    }
}
