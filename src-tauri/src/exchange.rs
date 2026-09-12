//! The community API seam: which server the app talks to and how it
//! sends. Everything that reaches the API - lookups, trade, routing,
//! knowledge, telemetry, the update manifest - resolves its base URL
//! through [`endpoint`] and sends through [`SendApi`].
//!
//! Until 2026-09-09 this module also downloaded, verified and hydrated
//! the community market baseline and the full-galaxy routing index into
//! a local database (EBEX products, chunked routing downloads, overlay
//! chains, consent, the six-hour update heartbeat). The maintainer ruled the
//! local store journal-only ("Local search should be limited to journal
//! data, that's it") and the API-only spec's B.4 cut all of it: every
//! search runs on the server, the bundled bubble index plots, the
//! journal is the commander's own truth.

use crate::state::AppState;
use tauri::State;

/// The community API every install uses out of the box. A fork or a
/// self-hosted server changes this one constant (and the updater
/// endpoint in tauri.conf.json); a development server goes in
/// Settings → System data, or EDDA_API_URL, without a rebuild.
pub const DEFAULT_COMMUNITY_API: &str = "https://api.edda-app.com";

/// Where the dev toggle points: a local `ed-api serve` on its default
/// bind.
pub const DEV_LOCAL_API: &str = "http://127.0.0.1:8787";

/// Precedence: EDDA_API_URL (a shell-level order beats a saved toggle) →
/// the dev-build "use local API" switch → the commander's saved override
/// (self-hosters) → the canonical server.
pub(crate) fn pick_endpoint(
    env: Option<String>,
    dev_local: bool,
    saved: Option<String>,
) -> Option<String> {
    env.or_else(|| dev_local.then(|| DEV_LOCAL_API.to_string()))
        .or(saved)
        .or_else(|| Some(DEFAULT_COMMUNITY_API.to_string()))
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

pub(crate) fn endpoint(state: &AppState) -> Option<String> {
    // Tests never reach the community server: without an explicit
    // EDDA_API_URL there is no endpoint, and every remote call fails fast
    // with `api_down` (B.4: the unit suite has no local data to fall back
    // to either, and must not load prod to compensate).
    if cfg!(test) {
        return std::env::var("EDDA_API_URL")
            .ok()
            .filter(|u| !u.trim().is_empty());
    }
    let (dev_local, saved) = {
        let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        (
            // Release builds ignore the toggle even if the config file
            // carries it (shared data dirs must not strand a commander
            // on a dead localhost).
            cfg!(debug_assertions) && config.dev_api_local == Some(true),
            config.community_api_url.clone(),
        )
    };
    pick_endpoint(std::env::var("EDDA_API_URL").ok(), dev_local, saved)
}

/// The dev-build API switch: which base the app talks to right now,
/// and whether the switch exists at all (release builds: it doesn't).
#[derive(Clone, Debug, serde::Serialize)]
pub struct DevApiStatus {
    pub available: bool,
    pub local: bool,
    pub local_url: String,
    /// What endpoint() resolves to with the current settings.
    pub effective: Option<String>,
}

pub(crate) fn dev_api_status(state: &AppState) -> DevApiStatus {
    let local = cfg!(debug_assertions)
        && state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dev_api_local
            == Some(true);
    DevApiStatus {
        available: cfg!(debug_assertions),
        local,
        local_url: DEV_LOCAL_API.to_owned(),
        effective: endpoint(state),
    }
}

#[tauri::command]
pub async fn dev_api_get(state: State<'_, AppState>) -> Result<DevApiStatus, String> {
    Ok(dev_api_status(&state))
}

#[tauri::command]
pub async fn dev_api_set(state: State<'_, AppState>, local: bool) -> Result<DevApiStatus, String> {
    if !cfg!(debug_assertions) {
        return Err("the API switch is a dev-build control".into());
    }
    {
        let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        config.dev_api_local = Some(local);
        config
            .save(&state.data_dir)
            .map_err(|error| error.to_string())?;
    }
    Ok(dev_api_status(&state))
}

/// How long to wait before retrying a 429, honouring `Retry-After`
/// (seconds) when the server sends one, else 3 s, then 6 s; `None` when
/// the retries are spent. Capped at 10 s so a burst-limited commander
/// waits a moment, never a minute.
pub fn retry_delay(retry_after: Option<&str>, attempt: u32) -> Option<std::time::Duration> {
    if attempt >= 2 {
        return None;
    }
    let secs = retry_after
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .unwrap_or(3.0 * f64::from(attempt + 1))
        .min(10.0);
    Some(std::time::Duration::from_secs_f64(secs))
}

/// `.send_api()` in place of `.send()` for calls to the community API:
/// a 429 (the per-install burst cap or hourly ceiling) is retried up to
/// twice after [`retry_delay`], quietly - with per-IP caps a commander
/// behind a shared NAT otherwise saw EDDA "randomly stop working"
/// (the assistant session, 2026-09-09). Anything else returns as it came.
pub trait SendApi {
    fn send_api(
        self,
    ) -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send;
}

impl SendApi for reqwest::RequestBuilder {
    async fn send_api(self) -> reqwest::Result<reqwest::Response> {
        let mut attempt = 0u32;
        let mut builder = self;
        loop {
            let again = builder.try_clone();
            let response = builder.send().await?;
            if response.status().as_u16() != 429 {
                return Ok(response);
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let (Some(next), Some(delay)) = (again, retry_delay(retry_after.as_deref(), attempt))
            else {
                return Ok(response);
            };
            tracing::info!(
                attempt,
                wait_ms = delay.as_millis() as u64,
                "API asked us to slow down (429); retrying quietly"
            );
            tokio::time::sleep(delay).await;
            builder = next;
            attempt += 1;
        }
    }
}

/// The blocking twin, for the sweeps that run on a plain thread.
pub trait SendApiBlocking {
    fn send_api(self) -> reqwest::Result<reqwest::blocking::Response>;
}

impl SendApiBlocking for reqwest::blocking::RequestBuilder {
    fn send_api(self) -> reqwest::Result<reqwest::blocking::Response> {
        let mut attempt = 0u32;
        let mut builder = self;
        loop {
            let again = builder.try_clone();
            let response = builder.send()?;
            if response.status().as_u16() != 429 {
                return Ok(response);
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let (Some(next), Some(delay)) = (again, retry_delay(retry_after.as_deref(), attempt))
            else {
                return Ok(response);
            };
            tracing::info!(
                attempt,
                wait_ms = delay.as_millis() as u64,
                "API asked us to slow down (429); retrying quietly"
            );
            std::thread::sleep(delay);
            builder = next;
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod send_api_tests {
    use super::*;

    #[test]
    fn retry_delay_honours_retry_after_and_gives_up_after_two() {
        assert_eq!(
            retry_delay(None, 0),
            Some(std::time::Duration::from_secs(3))
        );
        assert_eq!(
            retry_delay(None, 1),
            Some(std::time::Duration::from_secs(6))
        );
        assert_eq!(retry_delay(None, 2), None);
        assert_eq!(
            retry_delay(Some("2"), 0),
            Some(std::time::Duration::from_secs(2))
        );
        assert_eq!(
            retry_delay(Some("120"), 0),
            Some(std::time::Duration::from_secs(10)),
            "capped"
        );
        assert_eq!(
            retry_delay(Some("garbage"), 0),
            Some(std::time::Duration::from_secs(3))
        );
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    /// Out of the box the canonical server is used; a saved override or
    /// EDDA_API_URL wins over it; blank everywhere means not configured.
    #[test]
    fn endpoint_prefers_env_then_dev_toggle_then_override_then_canonical() {
        assert_eq!(
            pick_endpoint(
                Some("http://dev:1/".into()),
                true,
                Some("http://mine:2".into())
            ),
            Some("http://dev:1".into()),
            "a shell-level order beats the toggle"
        );
        assert_eq!(
            pick_endpoint(None, true, Some("http://mine:2".into())),
            Some(DEV_LOCAL_API.into()),
            "the dev toggle beats a saved override"
        );
        assert_eq!(
            pick_endpoint(None, false, Some("http://mine:2/".into())),
            Some("http://mine:2".into())
        );
        let canonical = pick_endpoint(None, false, None);
        if DEFAULT_COMMUNITY_API.is_empty() {
            assert_eq!(canonical, None, "no canonical server is live yet");
        } else {
            assert_eq!(canonical.as_deref(), Some(DEFAULT_COMMUNITY_API));
        }
        assert_eq!(
            pick_endpoint(Some("  ".into()), false, None),
            None,
            "blank is not a server"
        );
    }
}
