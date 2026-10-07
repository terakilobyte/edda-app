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
/// endpoint in tauri.conf.json); a development server is EDDA_API_URL in
/// the shell before launch, nothing else.
pub const DEFAULT_COMMUNITY_API: &str = "https://api.edda-app.com";

/// ONE knob (boss, 2026-10-07: "fold it into one knob, just the env var"):
/// `EDDA_API_URL` set in the shell before launch wins, else the canonical
/// server. The saved override and the dev-build toggle that used to sit
/// between them are gone -- two sessions spent an afternoon on plots that
/// a saved toggle had sent to a stale local server while everyone read
/// them as production's.
pub(crate) fn pick_endpoint(env: Option<String>) -> Option<String> {
    env.or_else(|| Some(DEFAULT_COMMUNITY_API.to_string()))
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

pub(crate) fn endpoint(_state: &AppState) -> Option<String> {
    // Tests never reach the community server: without an explicit
    // EDDA_API_URL there is no endpoint, and every remote call fails fast
    // with `api_down` (B.4: the unit suite has no local data to fall back
    // to either, and must not load prod to compensate).
    if cfg!(test) {
        return std::env::var("EDDA_API_URL").ok().filter(|u| !u.trim().is_empty());
    }
    pick_endpoint(std::env::var("EDDA_API_URL").ok())
}

/// Where the endpoint came from, for the Settings line.
pub fn endpoint_source() -> &'static str {
    if std::env::var("EDDA_API_URL").ok().is_some_and(|u| !u.trim().is_empty()) { "EDDA_API_URL" } else { "production" }
}

/// What the API the app is on says about itself, for Settings: the
/// endpoint, whether /healthz answered and how fast, and /readyz's checks.
/// 2026-10-07: the boss's dev app was on the WSL server by his own rule,
/// but that server had been built from a stale tree and every slow plot
/// was read as production's; the address alone did not say which server
/// it was, or whether it was even the one he thought.
#[derive(Debug, serde::Serialize)]
pub struct ApiProbe {
    pub endpoint: Option<String>,
    /// "EDDA_API_URL" when the shell set it, else "production".
    pub source: &'static str,
    pub healthy: bool,
    pub ms: u64,
    pub ready: Option<serde_json::Value>,
    /// GET /v1/version: {version, git, built_at, migrations_known} (ed-api
    /// since 2026-10-07; `version` is ed-api's own crate version, so lead
    /// with the sha and built_at). None on an older server.
    pub build: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn api_probe(state: State<'_, AppState>) -> Result<ApiProbe, String> {
    let Some(endpoint) = endpoint(&state) else {
        return Ok(ApiProbe { endpoint: None, source: endpoint_source(), healthy: false, ms: 0, ready: None, build: None, error: Some("no API configured".into()) });
    };
    let started = std::time::Instant::now();
    let health = state.http.get(format!("{endpoint}/healthz")).timeout(std::time::Duration::from_secs(5)).send().await;
    let ms = started.elapsed().as_millis() as u64;
    let (healthy, error) = match health {
        Ok(r) if r.status().is_success() => (true, None),
        Ok(r) => (false, Some(format!("healthz {}", r.status()))),
        Err(e) => (false, Some(e.to_string())),
    };
    let ready = if healthy {
        match state.http.get(format!("{endpoint}/readyz")).timeout(std::time::Duration::from_secs(5)).send().await {
            Ok(r) => r.json::<serde_json::Value>().await.ok(),
            Err(_) => None,
        }
    } else {
        None
    };
    let build = if healthy {
        match state.http.get(format!("{endpoint}/v1/version")).timeout(std::time::Duration::from_secs(5)).send().await {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
            _ => None,
        }
    } else {
        None
    };
    Ok(ApiProbe { endpoint: Some(endpoint), source: endpoint_source(), healthy, ms, ready, build, error })
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
/// (the assistant session, 2026-09-09). A connection that never opened
/// (DNS, refused, reset) or a gateway answering for the server (502,
/// 503, 504) is retried once after a beat (maintainer, 2026-09-27: "why
/// are we not auto retrying?"); a timeout is not, since the long lane
/// already waits 130 s. Anything else returns as it came.
pub trait SendApi {
    fn send_api(self) -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send;
}

impl SendApi for reqwest::RequestBuilder {
    async fn send_api(self) -> reqwest::Result<reqwest::Response> {
        let mut attempt = 0u32;
        let mut builder = self;
        loop {
            let again = builder.try_clone();
            let response = match builder.send().await {
                Ok(r) => r,
                Err(e) if attempt == 0 && e.is_connect() && again.is_some() => {
                    tracing::warn!(error = %e, "API connection failed; retrying once");
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    builder = again.unwrap();
                    attempt += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let code = response.status().as_u16();
            if attempt == 0 && matches!(code, 502 | 503 | 504) && again.is_some() {
                tracing::warn!(status = code, "API gateway error; retrying once");
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                builder = again.unwrap();
                attempt += 1;
                continue;
            }
            if code != 429 {
                return Ok(response);
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let (Some(next), Some(delay)) = (again, retry_delay(retry_after.as_deref(), attempt)) else {
                return Ok(response);
            };
            tracing::info!(attempt, wait_ms = delay.as_millis() as u64, "API asked us to slow down (429); retrying quietly");
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
            let (Some(next), Some(delay)) = (again, retry_delay(retry_after.as_deref(), attempt)) else {
                return Ok(response);
            };
            tracing::info!(attempt, wait_ms = delay.as_millis() as u64, "API asked us to slow down (429); retrying quietly");
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
        assert_eq!(retry_delay(None, 0), Some(std::time::Duration::from_secs(3)));
        assert_eq!(retry_delay(None, 1), Some(std::time::Duration::from_secs(6)));
        assert_eq!(retry_delay(None, 2), None);
        assert_eq!(retry_delay(Some("2"), 0), Some(std::time::Duration::from_secs(2)));
        assert_eq!(retry_delay(Some("120"), 0), Some(std::time::Duration::from_secs(10)), "capped");
        assert_eq!(retry_delay(Some("garbage"), 0), Some(std::time::Duration::from_secs(3)));
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    /// Out of the box the canonical server is used; EDDA_API_URL wins over
    /// it; blank means not configured. There is no other knob.
    #[test]
    fn endpoint_is_the_env_var_or_the_canonical_server() {
        assert_eq!(pick_endpoint(Some("http://dev:1/".into())), Some("http://dev:1".into()), "a shell-level order wins, trailing slash dropped");
        assert_eq!(pick_endpoint(None).as_deref(), Some(DEFAULT_COMMUNITY_API));
        assert_eq!(pick_endpoint(Some("  ".into())), None, "blank is not a server");
    }
}

