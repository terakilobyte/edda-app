//! Frontier auth deep-link SPIKE (maintainer, 2026-09-06: "if we can just
//! auth directly via the application, we don't need the server
//! trampoline. Let's test that.") — a pre-registered measurement, not
//! a feature. DEV BUILDS ONLY; the release build compiles none of it.
//!
//! Hypothesis under test: Frontier's auth server accepts
//! `redirect_uri=edda://auth` and Windows delivers the callback into
//! the RUNNING app via the deep-link plugin — making the https
//! trampoline unnecessary.
//!
//! Pre-registered PASS: /token 200, /decode customer_id == journal FID,
//! /profile 200, /fleetcarrier 204 (no carrier owned yet — doubling as
//! the 204-path proof). Recorded: status + ms per call, FID match,
//! whether an "open EDDA?" prompt appeared and whether a second
//! instance spawned (the tester reports those two by eye).
//!
//! Privacy discipline: tokens live in memory only and are dropped with
//! the process; nothing touches the keychain, telemetry, or logs —
//! log lines carry statuses and timings, never payloads or tokens.

/// The command registers in every build so generate_handler stays
/// static; the release body is a refusal and the core never compiles.
#[tauri::command]
pub async fn capi_spike_start(app: tauri::AppHandle) -> Result<String, String> {
    #[cfg(debug_assertions)]
    {
        dev::start(app).await
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        Err("the CAPI spike exists only in dev builds".into())
    }
}

#[cfg(debug_assertions)]
pub use dev::on_deep_link;

#[cfg(debug_assertions)]
mod dev {
    use base64::Engine as _;
    use sha2::Digest as _;

    struct Pending {
        verifier: String,
        state: String,
    }

    static PENDING: std::sync::Mutex<Option<Pending>> = std::sync::Mutex::new(None);

    fn random_b64url(strip_padding: bool) -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("os randomness");
        let engine = if strip_padding {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
        } else {
            // Frontier's token endpoint wants the verifier WITH its
            // trailing padding (field lore from the EDCD notes).
            base64::engine::general_purpose::URL_SAFE
        };
        engine.encode(bytes)
    }

    /// Step 1: open the authorize URL in the browser. The callback arrives
    /// via `edda://auth` (see `on_deep_link` wired in lib.rs).
    pub async fn start(app: tauri::AppHandle) -> Result<String, String> {
        // Compile-time or runtime: the dev shell's env works either way,
        // and a stale incremental build can't strand the tester.
        let Some(client_id) = option_env!("EDDA_CAPI_CLIENT_ID")
            .map(str::to_owned)
            .or_else(|| std::env::var("EDDA_CAPI_CLIENT_ID").ok())
        else {
            return Err("set EDDA_CAPI_CLIENT_ID in the dev shell before launching".into());
        };
        let verifier = random_b64url(false);
        let challenge = {
            let digest = sha2::Sha256::digest(verifier.as_bytes());
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
        };
        let state = random_b64url(true);
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            verifier,
            state: state.clone(),
        });
        let url = format!(
            "https://auth.frontierstore.net/auth?audience=frontier,steam,epic&scope=auth%20capi\
         &response_type=code&client_id={client_id}&code_challenge={challenge}\
         &code_challenge_method=S256&state={state}&redirect_uri=edda%3A%2F%2Fauth"
        );
        tauri_plugin_opener::OpenerExt::opener(&app)
            .open_url(url, None::<String>)
            .map_err(|e| e.to_string())?;
        Ok("Browser opened — complete the Frontier login. Results land in the dev log.".into())
    }

    /// Step 2: the deep link arrived. Exchange and probe, logging the five
    /// pre-registered numbers.
    pub fn on_deep_link(app: &tauri::AppHandle, url: &str) {
        let Ok(parsed) = reqwest::Url::parse(url) else {
            tracing::warn!("capi spike: unparseable deep link");
            return;
        };
        if parsed.scheme() != "edda" {
            return;
        }
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        let (Some(code), Some(state)) = (q.get("code"), q.get("state")) else {
            tracing::warn!("capi spike: deep link without code/state");
            return;
        };
        let Some(pending) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            tracing::warn!("capi spike: callback with nothing pending");
            return;
        };
        if pending.state != state.as_ref() {
            tracing::warn!("capi spike: STATE MISMATCH — refusing the code");
            return;
        }
        let code = code.to_string();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = exchange_and_probe(&app, &code, &pending.verifier).await {
                tracing::warn!(%error, "capi spike failed");
            }
        });
    }

    async fn exchange_and_probe(
        app: &tauri::AppHandle,
        code: &str,
        verifier: &str,
    ) -> Result<(), String> {
        use tauri::Manager as _;
        let state = app.state::<crate::state::AppState>();
        let client_id = option_env!("EDDA_CAPI_CLIENT_ID")
            .map(str::to_owned)
            .or_else(|| std::env::var("EDDA_CAPI_CLIENT_ID").ok())
            .unwrap_or_default();
        let agent = format!("EDCD-EDDA-{}", env!("CARGO_PKG_VERSION"));
        let started = std::time::Instant::now();
        let token_response = state
            .http
            .post("https://auth.frontierstore.net/token")
            .header("User-Agent", &agent)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id.as_str()),
                ("redirect_uri", "edda://auth"),
                ("code", code),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let token_status = token_response.status().as_u16();
        let token_ms = started.elapsed().as_millis() as u64;
        tracing::info!(status = token_status, ms = token_ms, "capi spike: /token");
        if token_status != 200 {
            return Err(format!("/token answered {token_status}"));
        }
        let token: serde_json::Value = token_response.json().await.map_err(|e| e.to_string())?;
        let bearer = token
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or("no access_token in the /token answer")?
            .to_owned();
        // Journal FID for the wrong-account check ("F1234..." → "1234...").
        let journal_fid: Option<String> = state.read_conn().ok().and_then(|conn| {
        conn.query_row(
            "SELECT json_extract(raw, '$.FID') FROM events WHERE event = 'Commander' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    });
        let get = |path: &'static str, bearer: String, agent: String| {
            let http = state.http.clone();
            async move {
                let started = std::time::Instant::now();
                let response = http
                    .get(format!("https://auth.frontierstore.net{path}"))
                    .bearer_auth(bearer)
                    .header("User-Agent", agent)
                    .send()
                    .await;
                (path, response, started.elapsed().as_millis() as u64)
            }
        };
        // /decode lives on the AUTH host; /profile and /fleetcarrier on CAPI.
        let (_, decode, decode_ms) = get("/decode", bearer.clone(), agent.clone()).await;
        let decode = decode.map_err(|e| e.to_string())?;
        let decode_status = decode.status().as_u16();
        let decoded: serde_json::Value = decode.json().await.unwrap_or_default();
        let customer_id = decoded.pointer("/usr/customer_id").and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        });
        let fid_matches = match (&customer_id, &journal_fid) {
            (Some(cid), Some(fid)) => Some(cid == fid.trim_start_matches(['F', 'f'])),
            _ => None,
        };
        tracing::info!(
            status = decode_status,
            ms = decode_ms,
            ?fid_matches,
            "capi spike: /decode"
        );
        for path in ["/profile", "/fleetcarrier"] {
            let started = std::time::Instant::now();
            let response = state
                .http
                .get(format!("https://companion.orerve.net{path}"))
                .bearer_auth(&bearer)
                .header("User-Agent", &agent)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            tracing::info!(
                status = response.status().as_u16(),
                ms = started.elapsed().as_millis() as u64,
                path,
                "capi spike: capi call"
            );
        }
        tracing::info!("capi spike: COMPLETE — tokens dropped with this task");
        Ok(())
    }
}
