use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceConfig {
    pub bind: SocketAddr,
    pub database_url: String,
    pub artifact_dir: PathBuf,
    pub eddn_relay: String,
    pub eddn_queue_capacity: usize,
    /// Where `ed-api ingest` serves its own /healthz, /readyz and
    /// /metrics (`EDDA_API_INGEST_BIND`, default 127.0.0.1:8788).
    pub ingest_bind: SocketAddr,
    /// Whether `ed-api serve` runs the EDDN feed in-process
    /// (`EDDA_API_EDDN_IN_SERVE`, default true). The box sets it false
    /// once `edda-eddn.service` owns the feed, so a serve swap no longer
    /// loses a minute of boards (measured 2026-09-09: ~65 s, ~250–300
    /// boards per restart).
    pub eddn_in_serve: bool,
}

/// `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off`, case-blind.
pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl ServiceConfig {
    pub fn from_env() -> Result<Self> {
        let bind = env::var("EDDA_API_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_owned())
            .parse()
            .context("EDDA_API_BIND must be an IP address and port")?;
        let database_url = env::var("DATABASE_URL")
            .context("DATABASE_URL must point to the EDDA PostgreSQL database")?;
        let artifact_dir = env::var_os("EDDA_API_ARTIFACT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".data/api/artifacts"));
        let eddn_relay =
            env::var("EDDA_API_EDDN_RELAY").unwrap_or_else(|_| ed_eddn::EDDN_RELAY.to_owned());
        let eddn_queue_capacity = env::var("EDDA_API_EDDN_QUEUE_CAPACITY")
            .unwrap_or_else(|_| "10000".to_owned())
            .parse()
            .context("EDDA_API_EDDN_QUEUE_CAPACITY must be a positive integer")?;
        anyhow::ensure!(
            eddn_queue_capacity > 0,
            "EDDA_API_EDDN_QUEUE_CAPACITY must be positive"
        );
        let ingest_bind = env::var("EDDA_API_INGEST_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8788".to_owned())
            .parse()
            .context("EDDA_API_INGEST_BIND must be an IP address and port")?;
        let eddn_in_serve = match env::var("EDDA_API_EDDN_IN_SERVE") {
            Ok(text) => parse_bool(&text).with_context(|| {
                format!("EDDA_API_EDDN_IN_SERVE must be true or false, got {text:?}")
            })?,
            Err(_) => true,
        };

        Ok(Self {
            bind,
            database_url,
            artifact_dir,
            eddn_relay,
            eddn_queue_capacity,
            ingest_bind,
            eddn_in_serve,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_serve_feed_switch_reads_the_usual_spellings() {
        for yes in ["true", "1", "yes", "ON", " True "] {
            assert_eq!(parse_bool(yes), Some(true), "{yes:?}");
        }
        for no in ["false", "0", "no", "off"] {
            assert_eq!(parse_bool(no), Some(false), "{no:?}");
        }
        assert_eq!(
            parse_bool("maybe"),
            None,
            "an unknown spelling is a startup error, never a silent default"
        );
    }
}
