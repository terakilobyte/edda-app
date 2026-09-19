//! The Frontier link: Frontier's Companion API (CAPI), client-side only.
//!
//! Ruled 2026-09-06 (docs/archive/superpowers/specs/2026-09-06-fleet-carrier-design.md
//! §4): OAuth2 authorization-code + PKCE in the app, the refresh token in
//! the OS keychain, the access token in memory, and every CAPI call made
//! by the app with the player's token over the player's connection. The
//! server never sees a token or a byte of CAPI data; telemetry may carry
//! `record_timing("capi", ..)` and a `capi_linked` flag and nothing else.
//!
//! Measured before building (the 2026-09-06 spike, a real account on
//! Windows): Frontier accepts `redirect_uri=edda://auth` and the deep
//! link reaches the running app -- /token 748 ms, /decode 185 ms with the
//! customer_id equal to the journal FID, /profile 985 ms, /fleetcarrier
//! 204 (no carrier owned then). This module is that spike grown up; the
//! paste-the-code box covers a machine with no scheme handler.
//!
//! What is fetched: `/profile` on link (and it verifies the login is this
//! journal's account); `/fleetcarrier` on link, on the carrier events the
//! journal writes, and on demand, with a 15-minute cooldown (EDCD asks
//! for no more than about one CAPI call a minute) -- the carrier's real
//! hold, tank, balance, orders and services, which the journal cannot
//! know (docs/ROADMAP.md, "A carrier's real inventory needs CAPI").

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest as _;
use tauri::Manager as _;

use crate::state::{secrets, AppState};

const AUTH_HOST: &str = "https://auth.frontierstore.net";
const CAPI_HOST: &str = "https://companion.orerve.net";
const REDIRECT: &str = "edda://auth";
/// Keychain slot for the refresh token (the one durable secret).
pub const REFRESH_SLOT: &str = "frontier_refresh_token";
/// EDCD: "try not to issue more than one query a minute"; a carrier does
/// not change faster than this, and the journal events that mean it did
/// bypass the cooldown.
pub const CARRIER_COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// `/fleetcarrier` is the slow call; it runs on its own task and never
/// holds up anything else.
const CAPI_TIMEOUT: Duration = Duration::from_secs(120);

fn client_id() -> Option<String> {
    option_env!("EDDA_CAPI_CLIENT_ID")
        .map(str::to_owned)
        .or_else(|| std::env::var("EDDA_CAPI_CLIENT_ID").ok())
        .filter(|s| !s.trim().is_empty())
}

fn user_agent() -> String {
    format!("EDCD-EDDA-{}", env!("CARGO_PKG_VERSION"))
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ------------------------------------------------------------------ state

struct Pending {
    verifier: String,
    state: String,
}

struct Access {
    token: String,
    expires_at: Instant,
}

/// Why the link is not usable, in the words the Settings card shows.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// No refresh token in the keychain.
    Unlinked,
    /// A refresh token exists (and the last refresh, if any, worked).
    Linked,
    /// The refresh token was refused: Frontier's tokens last about 25
    /// days, and linking on another machine invalidates this one.
    Expired,
    /// The login completed but belongs to a different account than this
    /// journal's FID. Tokens were discarded.
    WrongAccount,
    /// The build carries no client id (a source checkout without the
    /// secret); the card says so instead of offering a Link that fails.
    Unavailable,
}

#[derive(Default)]
pub struct CapiState {
    pending: Option<Pending>,
    access: Option<Access>,
    link: Option<LinkState>,
    /// `/fleetcarrier` answered 204: no carrier owned. Cleared by a
    /// CarrierBuy or a manual refresh, so a new owner is not stuck.
    no_carrier: bool,
    last_carrier_fetch: Option<Instant>,
    carrier_in_flight: bool,
    pub profile: Option<ProfileSummary>,
    pub last_error: Option<String>,
}

/// What `/profile` tells us that the journal does not know for sure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileSummary {
    pub commander: Option<String>,
    pub credits: Option<i64>,
    pub debt: Option<i64>,
    pub ship: Option<String>,
    pub ship_name: Option<String>,
    pub docked_at: Option<String>,
    pub system: Option<String>,
    pub fetched_at: String,
}

/// The carrier as Frontier reports it, folded for the card and the ship
/// computer. `hold` is per commodity (the wire lists one entry per unit).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CarrierLive {
    pub callsign: String,
    pub name: Option<String>,
    pub system: Option<String>,
    pub fuel_t: Option<i64>,
    pub balance_cr: Option<i64>,
    pub reserved_cr: Option<i64>,
    pub state: Option<String>,
    pub docking_access: Option<String>,
    pub notorious_access: Option<bool>,
    pub capacity: Option<Value>,
    pub current_jump: Option<String>,
    pub hold: Vec<HoldLine>,
    pub hold_t: i64,
    pub hold_value_cr: i64,
    pub market: Option<Value>,
    pub sales: Vec<Value>,
    pub purchases: Vec<Value>,
    pub services: Vec<String>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HoldLine {
    pub commodity: String,
    pub name: String,
    pub tonnes: i64,
    pub value_cr: i64,
    pub stolen_t: i64,
    pub mission_t: i64,
}

/// What the Settings card and the Ships tab read.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub link: LinkState,
    pub profile: Option<ProfileSummary>,
    pub carrier: Option<CarrierLive>,
    pub no_carrier: bool,
    pub last_error: Option<String>,
    pub cooldown_secs_left: u64,
}

// ------------------------------------------------------------------ pure parts

fn random_b64url(strip_padding: bool) -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("os randomness");
    if strip_padding {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    } else {
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    }
}

fn challenge_for(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// The authorize URL for a fresh PKCE pair (EDMC's audience string so
/// Steam and Epic logins complete the Frontier page).
pub fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
    format!(
        "{AUTH_HOST}/auth?audience=frontier,steam,epic&scope=auth%20capi\
         &response_type=code&client_id={client_id}&code_challenge={challenge}\
         &code_challenge_method=S256&state={state}&redirect_uri=edda%3A%2F%2Fauth"
    )
}

/// The code and state out of what came back: the `edda://auth?...` deep
/// link, the same URL pasted from the browser's address bar, or a bare
/// `code=...&state=...` query.
pub fn parse_callback(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let query = if let Ok(url) = reqwest::Url::parse(text) {
        if url.scheme() != "edda" && url.scheme() != "http" && url.scheme() != "https" {
            return None;
        }
        url.query()?.to_owned()
    } else {
        text.trim_start_matches('?').to_owned()
    };
    let pairs: BTreeMap<String, String> = reqwest::Url::parse(&format!("edda://auth?{query}"))
        .ok()?
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    Some((pairs.get("code")?.clone(), pairs.get("state")?.clone()))
}

/// `/decode` gives `usr.customer_id`; the journal's `Commander` event
/// carries `FID` = `F<customer_id>`. Equal means the login is this
/// journal's account.
pub fn fid_matches(customer_id: Option<&str>, journal_fid: Option<&str>) -> Option<bool> {
    Some(customer_id? == journal_fid?.trim_start_matches(['F', 'f']))
}

/// Frontier hex-encodes the vanity name.
fn decode_vanity(hex: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Fold a `/fleetcarrier` 200 body into [`CarrierLive`].
pub fn carrier_from_json(v: &Value, fetched_at: &str) -> Option<CarrierLive> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
    let i = |p: &str| {
        v.pointer(p)
            .and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|t| t.parse().ok())))
    };
    let callsign = s("/name/callsign")?;
    let name = s("/name/filteredVanityName")
        .or_else(|| s("/name/vanityName"))
        .and_then(|h| decode_vanity(&h));

    let mut hold: BTreeMap<String, HoldLine> = BTreeMap::new();
    for item in v.pointer("/cargo").and_then(Value::as_array).into_iter().flatten() {
        let Some(commodity) = item.get("commodity").and_then(Value::as_str) else { continue };
        let qty = item.get("qty").and_then(Value::as_i64).unwrap_or(1).max(0);
        let value = item.get("value").and_then(Value::as_i64).unwrap_or(0);
        let stolen = item.get("stolen").and_then(Value::as_bool).unwrap_or(false);
        let mission = item.get("mission").and_then(Value::as_bool).unwrap_or(false);
        let line = hold.entry(commodity.to_ascii_lowercase()).or_insert_with(|| HoldLine {
            commodity: commodity.to_ascii_lowercase(),
            name: item
                .get("locName")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(commodity)
                .to_owned(),
            tonnes: 0,
            value_cr: 0,
            stolen_t: 0,
            mission_t: 0,
        });
        line.tonnes += qty;
        line.value_cr += value * qty;
        if stolen {
            line.stolen_t += qty;
        }
        if mission {
            line.mission_t += qty;
        }
    }
    let mut hold: Vec<HoldLine> = hold.into_values().collect();
    hold.sort_by(|a, b| b.tonnes.cmp(&a.tonnes).then_with(|| a.name.cmp(&b.name)));
    let hold_t = hold.iter().map(|l| l.tonnes).sum();
    let hold_value_cr = hold.iter().map(|l| l.value_cr).sum();

    let services: Vec<String> = v
        .pointer("/market/services")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, on)| on.as_str().map(|s| s == "ok").unwrap_or(false) || on.as_bool().unwrap_or(false))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let list = |p: &str| v.pointer(p).and_then(Value::as_array).cloned().unwrap_or_default();

    Some(CarrierLive {
        callsign,
        name,
        system: s("/currentStarSystem"),
        fuel_t: i("/fuel"),
        balance_cr: i("/finance/bankBalance").or_else(|| i("/balance")),
        reserved_cr: i("/finance/bankReservedBalance"),
        state: s("/state"),
        docking_access: s("/dockingAccess"),
        notorious_access: v.pointer("/notoriousAccess").and_then(Value::as_bool),
        capacity: v.pointer("/capacity").cloned(),
        current_jump: s("/itinerary/currentJump"),
        hold,
        hold_t,
        hold_value_cr,
        market: v.pointer("/marketFinances").cloned(),
        sales: list("/orders/commodities/sales"),
        purchases: list("/orders/commodities/purchases"),
        services,
        fetched_at: fetched_at.to_owned(),
    })
}

fn profile_from_json(v: &Value, fetched_at: &str) -> ProfileSummary {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
    let i = |p: &str| v.pointer(p).and_then(Value::as_i64);
    ProfileSummary {
        commander: s("/commander/name"),
        credits: i("/commander/credits"),
        debt: i("/commander/debt"),
        ship: s("/ship/name"),
        ship_name: s("/ship/shipName"),
        docked_at: s("/ship/station/name").or_else(|| s("/lastStarport/name")),
        system: s("/ship/starsystem/name").or_else(|| s("/lastSystem/name")),
        fetched_at: fetched_at.to_owned(),
    }
}

// ------------------------------------------------------------------ the flow

fn capi(app: &tauri::AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

/// A write on the store, under its lock (the read pool is read-only).
fn with_store<T>(state: &AppState, f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let guard = state.store.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.conn())
}

fn set_link(state: &AppState, link: LinkState) {
    state.capi.lock().unwrap_or_else(|e| e.into_inner()).link = Some(link);
}

fn emit_state(app: &tauri::AppHandle) {
    let status = status(&capi(app));
    use tauri::Emitter as _;
    let _ = app.emit(crate::events::CAPI_STATE, &status);
}

/// Where the link stands, without touching the network.
pub fn status(state: &AppState) -> Status {
    let mut s = state.capi.lock().unwrap_or_else(|e| e.into_inner());
    if s.link.is_none() {
        s.link = Some(if client_id().is_none() {
            LinkState::Unavailable
        } else if secrets::get_key(REFRESH_SLOT).is_some() {
            LinkState::Linked
        } else {
            LinkState::Unlinked
        });
    }
    let carrier = state
        .with_read(|st| ed_store::carrier_capi::load(st.conn()))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_value::<CarrierLive>(json).ok());
    let cooldown_secs_left = s
        .last_carrier_fetch
        .map(|t| CARRIER_COOLDOWN.saturating_sub(t.elapsed()).as_secs())
        .unwrap_or(0);
    Status {
        link: s.link.clone().unwrap_or(LinkState::Unlinked),
        profile: s.profile.clone(),
        carrier,
        no_carrier: s.no_carrier,
        last_error: s.last_error.clone(),
        cooldown_secs_left,
    }
}

/// Step 1: open Frontier's login in the browser. The callback arrives as
/// `edda://auth?code&state` (or is pasted into the Settings card).
pub fn link_start(app: &tauri::AppHandle) -> Result<String, String> {
    let Some(client_id) = client_id() else {
        return Err("this build carries no Frontier client id".into());
    };
    let verifier = random_b64url(false);
    let state = random_b64url(true);
    let url = authorize_url(&client_id, &challenge_for(&verifier), &state);
    {
        let st = capi(app);
        let mut s = st.capi.lock().unwrap_or_else(|e| e.into_inner());
        s.pending = Some(Pending { verifier, state });
        s.last_error = None;
    }
    tauri_plugin_opener::OpenerExt::opener(app)
        .open_url(url, None::<String>)
        .map_err(|e| e.to_string())?;
    Ok("Your browser has the Frontier login. It may ask permission to open EDDA when you finish.".into())
}

/// The deep link arrived while the app was running.
pub fn on_deep_link(app: &tauri::AppHandle, url: &str) {
    if !url.starts_with("edda://auth") {
        return;
    }
    let app = app.clone();
    let url = url.to_owned();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = link_with_callback(&app, &url).await {
            tracing::warn!(%error, "frontier link: callback refused");
            capi(&app).capi.lock().unwrap_or_else(|e| e.into_inner()).last_error = Some(error);
            emit_state(&app);
        }
    });
}

/// Step 2, from the deep link or the paste box: check the state, redeem
/// the code, verify the account, fetch the profile and the carrier.
pub async fn link_with_callback(app: &tauri::AppHandle, text: &str) -> Result<Status, String> {
    let (code, state) = parse_callback(text).ok_or("that is not a Frontier login callback (no code)")?;
    let verifier = {
        let st = capi(app);
        let mut s = st.capi.lock().unwrap_or_else(|e| e.into_inner());
        let pending = s.pending.take().ok_or("no login in progress — press Link first")?;
        if pending.state != state {
            return Err("the callback does not match the login EDDA started (state mismatch); press Link again".into());
        }
        pending.verifier
    };
    let st = capi(app);
    let tokens = token_request(&st, &[
        ("grant_type", "authorization_code"),
        ("client_id", &client_id().unwrap_or_default()),
        ("redirect_uri", REDIRECT),
        ("code", &code),
        ("code_verifier", &verifier),
    ])
    .await?;
    // The wrong-account check before anything is kept.
    let customer_id = decode_customer_id(&st, &tokens.access).await;
    let journal_fid = st.with_read(|s| journal_fid(s.conn()));
    if fid_matches(customer_id.as_deref(), journal_fid.as_deref()) == Some(false) {
        set_link(&st, LinkState::WrongAccount);
        emit_state(app);
        return Err("that Frontier login is not the account this journal belongs to; nothing was kept".into());
    }
    secrets::set_key(REFRESH_SLOT, &tokens.refresh)?;
    {
        let mut s = st.capi.lock().unwrap_or_else(|e| e.into_inner());
        s.access = Some(Access { token: tokens.access.clone(), expires_at: Instant::now() + tokens.lifetime });
        s.link = Some(LinkState::Linked);
        s.no_carrier = false;
        s.last_error = None;
    }
    tracing::info!("frontier link: linked");
    let _ = fetch_profile(&st).await;
    emit_state(app);
    // The carrier on its own task: /fleetcarrier is the slow one.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = fetch_carrier(&handle, true).await;
    });
    Ok(status(&st))
}

/// Forget the link: the keychain slot, the access token, the cached
/// carrier. Nothing else knows the account existed.
pub fn unlink(app: &tauri::AppHandle) -> Result<Status, String> {
    let st = capi(app);
    secrets::clear_key(REFRESH_SLOT)?;
    {
        let mut s = st.capi.lock().unwrap_or_else(|e| e.into_inner());
        s.access = None;
        s.pending = None;
        s.profile = None;
        s.no_carrier = false;
        s.last_error = None;
        s.link = Some(LinkState::Unlinked);
    }
    let _ = with_store(&st, ed_store::carrier_capi::clear);
    tracing::info!("frontier link: unlinked");
    emit_state(app);
    Ok(status(&st))
}

struct Tokens {
    access: String,
    refresh: String,
    lifetime: Duration,
}

async fn token_request(state: &AppState, form: &[(&str, &str)]) -> Result<Tokens, String> {
    let started = Instant::now();
    let response = state
        .http
        .post(format!("{AUTH_HOST}/token"))
        .header("User-Agent", user_agent())
        .form(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Frontier auth unreachable: {e}"))?;
    let code = response.status().as_u16();
    crate::telemetry::record_timing("capi", started.elapsed().as_millis(), code == 200);
    tracing::info!(status = code, ms = started.elapsed().as_millis() as u64, "frontier link: /token");
    if code != 200 {
        return Err(format!("Frontier auth answered {code}"));
    }
    let v: Value = response.json().await.map_err(|e| e.to_string())?;
    let access = v.get("access_token").and_then(Value::as_str).ok_or("no access token in Frontier's answer")?.to_owned();
    let refresh = v.get("refresh_token").and_then(Value::as_str).ok_or("no refresh token in Frontier's answer")?.to_owned();
    // Expire a minute early rather than race the server's clock.
    let lifetime = Duration::from_secs(v.get("expires_in").and_then(Value::as_u64).unwrap_or(4 * 3600).saturating_sub(60));
    Ok(Tokens { access, refresh, lifetime })
}

/// A valid access token: the one in memory, or a fresh one from the
/// keychain's refresh token. A refused refresh is the "re-link" state.
async fn ensure_access(state: &AppState) -> Result<String, String> {
    if let Some(a) = state.capi.lock().unwrap_or_else(|e| e.into_inner()).access.as_ref() {
        if a.expires_at > Instant::now() {
            return Ok(a.token.clone());
        }
    }
    let Some(refresh) = secrets::get_key(REFRESH_SLOT) else {
        set_link(state, LinkState::Unlinked);
        return Err("not linked".into());
    };
    let Some(client_id) = client_id() else {
        set_link(state, LinkState::Unavailable);
        return Err("this build carries no Frontier client id".into());
    };
    match token_request(state, &[("grant_type", "refresh_token"), ("client_id", &client_id), ("refresh_token", &refresh)]).await {
        Ok(tokens) => {
            secrets::set_key(REFRESH_SLOT, &tokens.refresh)?;
            let mut s = state.capi.lock().unwrap_or_else(|e| e.into_inner());
            s.access = Some(Access { token: tokens.access.clone(), expires_at: Instant::now() + tokens.lifetime });
            s.link = Some(LinkState::Linked);
            Ok(tokens.access)
        }
        Err(e) => {
            // 25 days, or linked elsewhere: the card says re-link, quietly.
            set_link(state, LinkState::Expired);
            Err(format!("the Frontier link has expired — link again ({e})"))
        }
    }
}

async fn capi_get(state: &AppState, host: &str, path: &str, bearer: &str) -> Result<(u16, Value), String> {
    let started = Instant::now();
    let response = state
        .http
        .get(format!("{host}{path}"))
        .bearer_auth(bearer)
        .header("User-Agent", user_agent())
        .timeout(CAPI_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Frontier unreachable: {e}"))?;
    let code = response.status().as_u16();
    let ms = started.elapsed().as_millis();
    crate::telemetry::record_timing("capi", ms, (200..300).contains(&code));
    tracing::info!(status = code, ms = ms as u64, path, "frontier link: capi");
    let body = if code == 204 { Value::Null } else { response.json().await.unwrap_or(Value::Null) };
    Ok((code, body))
}

async fn decode_customer_id(state: &AppState, bearer: &str) -> Option<String> {
    let (code, v) = capi_get(state, AUTH_HOST, "/decode", bearer).await.ok()?;
    if code != 200 {
        return None;
    }
    v.pointer("/usr/customer_id")
        .and_then(|c| c.as_str().map(str::to_owned).or_else(|| c.as_i64().map(|n| n.to_string())))
}

fn journal_fid(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        "SELECT json_extract(raw, '$.FID') FROM events WHERE event = 'Commander' ORDER BY ts DESC, file DESC, offset DESC LIMIT 1",
        [],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

pub async fn fetch_profile(state: &AppState) -> Result<ProfileSummary, String> {
    let bearer = ensure_access(state).await?;
    let (code, v) = capi_get(state, CAPI_HOST, "/profile", &bearer).await?;
    if code != 200 {
        return Err(format!("/profile answered {code}"));
    }
    let profile = profile_from_json(&v, &now_iso());
    state.capi.lock().unwrap_or_else(|e| e.into_inner()).profile = Some(profile.clone());
    Ok(profile)
}

/// `/fleetcarrier`, honouring the cooldown unless `force` (a carrier
/// event or the Refresh button). 204 remembers "no carrier" until a
/// CarrierBuy or a forced fetch. The result is cached in the store so
/// the card survives a restart, with the time it was fetched.
pub async fn fetch_carrier(app: &tauri::AppHandle, force: bool) -> Result<Option<CarrierLive>, String> {
    let st = capi(app);
    {
        let mut s = st.capi.lock().unwrap_or_else(|e| e.into_inner());
        if s.carrier_in_flight {
            return Err("a carrier fetch is already running".into());
        }
        if !force {
            if s.no_carrier {
                return Ok(None);
            }
            if s.last_carrier_fetch.is_some_and(|t| t.elapsed() < CARRIER_COOLDOWN) {
                return Err("carrier fetched recently; the cooldown is 15 minutes".into());
            }
        }
        s.carrier_in_flight = true;
    }
    let result = fetch_carrier_inner(&st).await;
    {
        let mut s = st.capi.lock().unwrap_or_else(|e| e.into_inner());
        s.carrier_in_flight = false;
        s.last_carrier_fetch = Some(Instant::now());
        match &result {
            Ok(None) => s.no_carrier = true,
            Ok(Some(_)) => s.no_carrier = false,
            Err(e) => s.last_error = Some(e.clone()),
        }
    }
    emit_state(app);
    result
}

async fn fetch_carrier_inner(st: &AppState) -> Result<Option<CarrierLive>, String> {
    let bearer = ensure_access(st).await?;
    let (code, v) = capi_get(st, CAPI_HOST, "/fleetcarrier", &bearer).await?;
    match code {
        204 => {
            let _ = with_store(st, ed_store::carrier_capi::clear);
            Ok(None)
        }
        200 => {
            let fetched_at = now_iso();
            let live = carrier_from_json(&v, &fetched_at).ok_or("Frontier's carrier answer had no callsign")?;
            let json = serde_json::to_value(&live).map_err(|e| e.to_string())?;
            with_store(st, |conn| ed_store::carrier_capi::save(conn, &live.callsign, &fetched_at, &json))
                .map_err(|e| e.to_string())?;
            tracing::info!(hold_lines = live.hold.len(), hold_t = live.hold_t, "frontier link: carrier cached");
            Ok(Some(live))
        }
        401 => {
            set_link(st, LinkState::Expired);
            Err("Frontier refused the token — link again".into())
        }
        other => Err(format!("/fleetcarrier answered {other}")),
    }
}

/// The journal wrote something that changes the carrier (CarrierStats,
/// CarrierBuy, CarrierTradeOrder, CarrierDepositFuel): refresh, bypassing
/// the cooldown, if linked. Fire-and-forget from the watcher.
pub fn on_carrier_event(app: &tauri::AppHandle, event: &str) {
    if secrets::get_key(REFRESH_SLOT).is_none() {
        return;
    }
    let app = app.clone();
    let event = event.to_owned();
    tauri::async_runtime::spawn(async move {
        if event == "CarrierBuy" {
            capi(&app).capi.lock().unwrap_or_else(|e| e.into_inner()).no_carrier = false;
        }
        if let Err(error) = fetch_carrier(&app, true).await {
            tracing::debug!(%error, event, "frontier link: carrier refresh skipped");
        }
    });
}

/// Held by AppState.
pub type CapiHandle = Mutex<CapiState>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorize_url_is_pkce_s256_to_the_deep_link() {
        let url = authorize_url("cid", "chal", "st8");
        assert!(url.starts_with("https://auth.frontierstore.net/auth?"));
        for needle in ["audience=frontier,steam,epic", "scope=auth%20capi", "client_id=cid", "code_challenge=chal", "code_challenge_method=S256", "state=st8", "redirect_uri=edda%3A%2F%2Fauth"] {
            assert!(url.contains(needle), "{needle} in {url}");
        }
        // RFC 7636: challenge = BASE64URL(SHA256(verifier)), no padding.
        assert_eq!(challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let v = random_b64url(false);
        assert!(v.len() >= 43, "verifier long enough for RFC 7636");
        assert_ne!(random_b64url(true), random_b64url(true));
    }

    #[test]
    fn the_callback_is_read_from_a_deep_link_a_pasted_url_or_a_bare_query() {
        let want = Some(("abc".to_string(), "xyz".to_string()));
        assert_eq!(parse_callback("edda://auth?code=abc&state=xyz"), want);
        assert_eq!(parse_callback("  edda://auth?state=xyz&code=abc  "), want);
        assert_eq!(parse_callback("code=abc&state=xyz"), want);
        assert_eq!(parse_callback("?code=abc&state=xyz"), want);
        assert_eq!(parse_callback("https://api.edda-app.com/v1/auth/frontier/callback?code=abc&state=xyz"), want, "the registered https trampoline URI, pasted");
        assert_eq!(parse_callback("edda://auth?code=abc"), None, "no state, no deal");
        assert_eq!(parse_callback("ftp://x?code=a&state=b"), None);
        assert_eq!(parse_callback(""), None);
    }

    #[test]
    fn the_login_must_be_this_journals_account() {
        assert_eq!(fid_matches(Some("1234"), Some("F1234")), Some(true));
        assert_eq!(fid_matches(Some("1234"), Some("F9999")), Some(false));
        assert_eq!(fid_matches(Some("1234"), None), None, "no Commander event yet: cannot check, do not refuse");
        assert_eq!(fid_matches(None, Some("F1234")), None);
    }

    /// The EDCD-documented /fleetcarrier shape: cargo is one entry per
    /// unit, the vanity name is hex, services are a map of status words.
    #[test]
    fn a_fleetcarrier_answer_folds_into_the_card() {
        let v: Value = serde_json::json!({
            "name": {"callsign": "Q6Z-66L", "vanityName": "54656E7A696E67204E6F72676179", "filteredVanityName": "54656E7A696E67204E6F72676179"},
            "currentStarSystem": "HIP 90112",
            "balance": "9734563753",
            "fuel": "587",
            "state": "normalOperation",
            "dockingAccess": "all",
            "notoriousAccess": true,
            "capacity": {"freeSpace": 19391, "cargoForSale": 100, "cargoNotForSale": 5509},
            "itinerary": {"currentJump": null, "completed": []},
            "marketFinances": {"cargoTotalValue": 12345, "allTimeProfit": 6789, "numCommodsForSale": 3, "numCommodsPurchaseOrders": 1},
            "finance": {"bankBalance": 9734563753i64, "bankReservedBalance": 100000000},
            "cargo": [
                {"commodity": "Tritium", "mission": false, "qty": 1, "value": 51000, "stolen": false, "locName": "Tritium"},
                {"commodity": "Tritium", "mission": false, "qty": 1, "value": 51000, "stolen": false, "locName": "Tritium"},
                {"commodity": "LiquidOxygen", "mission": false, "qty": 1, "value": 300, "stolen": false, "locName": "Liquid Oxygen"},
                {"commodity": "Gold", "mission": true, "qty": 1, "value": 9000, "stolen": true, "locName": "Gold"}
            ],
            "orders": {"commodities": {"sales": [{"name": "tritium", "stock": 2, "price": 60000, "blackmarket": false}], "purchases": []}},
            "market": {"services": {"commodities": "ok", "refuel": "ok", "shipyard": "off", "repair": "ok"}}
        });
        let c = carrier_from_json(&v, "2026-09-19T03:00:00Z").expect("a carrier");
        assert_eq!(c.callsign, "Q6Z-66L");
        assert_eq!(c.name.as_deref(), Some("Tenzing Norgay"), "hex-encoded vanity name decoded");
        assert_eq!(c.system.as_deref(), Some("HIP 90112"));
        assert_eq!((c.fuel_t, c.balance_cr, c.reserved_cr), (Some(587), Some(9_734_563_753), Some(100_000_000)));
        assert_eq!(c.hold.iter().map(|l| (l.name.as_str(), l.tonnes)).collect::<Vec<_>>(), [("Tritium", 2), ("Gold", 1), ("Liquid Oxygen", 1)], "one wire entry per unit, folded and sorted by tonnes");
        assert_eq!((c.hold_t, c.hold_value_cr), (4, 111_300));
        let gold = c.hold.iter().find(|l| l.name == "Gold").unwrap();
        assert_eq!((gold.stolen_t, gold.mission_t), (1, 1));
        assert_eq!(c.services, vec!["commodities", "refuel", "repair"], "only services reporting ok");
        assert_eq!(c.sales.len(), 1);
        assert_eq!(c.current_jump, None);
        assert_eq!(c.fetched_at, "2026-09-19T03:00:00Z");
        // Round-trips through the cache.
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(serde_json::from_value::<CarrierLive>(json).unwrap(), c);
    }

    #[test]
    fn a_profile_answer_yields_the_summary() {
        let v: Value = serde_json::json!({
            "commander": {"name": "Waldorf", "credits": 3183856809i64, "debt": 0},
            "ship": {"name": "Explorer_NX", "shipName": "Colonia Bus", "station": {"name": "K3G-43N"}, "starsystem": {"name": "HIP 90112"}},
            "lastSystem": {"name": "HIP 90112"}
        });
        let p = profile_from_json(&v, "t");
        assert_eq!(p.commander.as_deref(), Some("Waldorf"));
        assert_eq!(p.credits, Some(3_183_856_809));
        assert_eq!(p.ship_name.as_deref(), Some("Colonia Bus"));
        assert_eq!(p.docked_at.as_deref(), Some("K3G-43N"));
        assert_eq!(p.system.as_deref(), Some("HIP 90112"));
    }
}
