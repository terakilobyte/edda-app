use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use ed_api::{
    cli::{parse_command, Command},
    config::ServiceConfig,
    database_pool, http, hydration, routing, snapshot,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let command = parse_command(env::args().skip(1).collect())?;
    let config = ServiceConfig::from_env()?;

    match command {
        Command::Serve => serve(config).await,
        Command::HydrateFixture(fixture) => {
            let pool = database_pool(&config).await?;
            let result = hydration::hydrate_fixture(&pool, &fixture).await?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        Command::HydrateSpansh(dump) => {
            let pool = database_pool(&config).await?;
            let result = hydration::hydrate_spansh(&pool, &dump).await?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        Command::HydrateEdsmBodies(dump) => {
            let pool = database_pool(&config).await?;
            let result = ed_api::stars::hydrate_edsm_bodies(&pool, &dump).await?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        Command::HydrateFdevIds(csv) => {
            let pool = database_pool(&config).await?;
            let written = match csv {
                Some(csv) => ed_api::fdev_ids::hydrate_commodities(&pool, &csv).await?,
                None => ed_api::fdev_ids::hydrate_builtin(&pool).await?,
            };
            println!("{{\"commodities_written\":{written}}}");
            Ok(())
        }
        Command::Ingest => {
            let pool = database_pool(&config).await?;
            ed_api::ingest::run(config, pool).await
        }
        Command::PublishCommunity => publish_community(config).await,
        Command::PublishMarketDaily => {
            let pool = database_pool(&config).await?;
            tokio::fs::create_dir_all(&config.artifact_dir).await?;
            let publication =
                ed_api::snapshot::publish_market_daily(&pool, &config.artifact_dir).await?;
            println!("{}", serde_json::to_string(&publication)?);
            Ok(())
        }
        Command::PublishStars => {
            let pool = database_pool(&config).await?;
            tokio::fs::create_dir_all(&config.artifact_dir).await?;
            let publication = ed_api::stars::publish_stars(&pool, &config.artifact_dir).await?;
            println!("{}", serde_json::to_string(&publication)?);
            Ok(())
        }
        Command::BuildRouting {
            source,
            artifact_dir,
        } => build_routing(config, source, artifact_dir).await,
        Command::AdoptRouting {
            prebuilt,
            artifact_dir,
        } => {
            let pool = database_pool(&config).await?;
            let artifact_dir = artifact_dir.unwrap_or(config.artifact_dir);
            tokio::fs::create_dir_all(&artifact_dir).await?;
            let publication =
                routing::adopt_routing_recorded(&pool, &artifact_dir, &prebuilt).await?;
            println!("{}", serde_json::to_string(&publication)?);
            Ok(())
        }
        Command::ReconcileRouting { artifact_dir } => {
            let pool = database_pool(&config).await?;
            let artifact_dir = artifact_dir.unwrap_or(config.artifact_dir);
            match ed_api::reconcile::reconcile_routing(&pool, &artifact_dir).await? {
                Some(publication) => println!("{}", serde_json::to_string(&publication)?),
                None => println!("{{\"published\":false}}"),
            }
            Ok(())
        }
    }
}

async fn publish_community(config: ServiceConfig) -> Result<()> {
    let pool = database_pool(&config).await?;
    tokio::fs::create_dir_all(&config.artifact_dir).await?;
    let publication = snapshot::publish_community(&pool, &config.artifact_dir).await?;
    println!("{}", serde_json::to_string(&publication)?);
    Ok(())
}

async fn build_routing(
    config: ServiceConfig,
    source: PathBuf,
    artifact_dir: Option<PathBuf>,
) -> Result<()> {
    let pool = database_pool(&config).await?;
    let artifact_dir = artifact_dir.unwrap_or(config.artifact_dir);
    tokio::fs::create_dir_all(&artifact_dir).await?;
    let publication = routing::publish_routing(&pool, &artifact_dir, &source).await?;
    println!("{}", serde_json::to_string(&publication)?);
    Ok(())
}

/// Planner threads run below normal priority so Postgres and the web
/// tier keep their share of the cores while a plot fans out. Linux only;
/// the desktop client does the Windows equivalent in ed_input.
fn planner_thread_start() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain libc call on the calling thread's own id; nice +10.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}

async fn serve(config: ServiceConfig) -> Result<()> {
    // The planner pool, sized like the desktop client's (cores − 2) and
    // niced. ed-api never called this before 2026-09-07: rayon's default
    // pool ran every core at normal priority, so two fanning plots read
    // 99.5 % CPU and a 253 ms trade search waited 15 s for a core (load
    // bench, API-only spec Phase A step 1).
    let planner_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(2)
        .max(1);
    ed_galaxy::init_thread_pool(planner_thread_start);
    tracing::info!(threads = planner_threads, nice = 10, "planner pool sized");
    // The recorder goes in before anything can emit; every counter
    // fired earlier would vanish into the pre-recorder void.
    let metrics_handle = ed_api::metrics::install()?;
    let pool = database_pool(&config).await?;
    // The EDDN feed rides in-process only until `edda-eddn.service`
    // owns it (EDDA_API_EDDN_IN_SERVE=false): then a serve swap loses
    // no boards. The gauges task is held either way; with no feed here
    // it simply has no queue to report.
    let (eddn_sender, eddn_receiver) = tokio::sync::mpsc::channel(config.eddn_queue_capacity);
    tokio::spawn(ed_api::metrics::run_gauges(
        pool.clone(),
        eddn_sender.downgrade(),
    ));
    if config.eddn_in_serve {
        tokio::spawn(ed_api::eddn::run_writer(pool.clone(), eddn_receiver));
        let relay = config.eddn_relay.clone();
        tokio::spawn(async move {
            if let Err(error) = ed_eddn::live::run_to_channel(&relay, eddn_sender).await {
                tracing::error!(%error, "EDDN feed stopped");
            }
        });
        tracing::info!(eddn = "embedded", "serve: EDDN feed runs in this process");
    } else {
        drop(eddn_sender);
        drop(eddn_receiver);
        tracing::info!(eddn = "external", "serve: EDDN feed is edda-eddn.service's");
    }
    // Systems clients asked about that the store cannot answer: EDSM, one
    // request a second, for as long as the server runs.
    tokio::spawn(ed_api::stars::run_edsm_lookups(
        pool.clone(),
        reqwest::Client::new(),
    ));
    tokio::fs::create_dir_all(&config.artifact_dir).await?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("failed to bind {}", config.bind))?;
    tracing::info!(bind = %config.bind, "EDDA API listening");

    axum::serve(
        listener,
        http::router(http::AppState::new(
            pool,
            config.artifact_dir,
            metrics_handle,
        )),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("HTTP server failed")
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal");
    }
}

fn init_tracing() {
    // The whole ingest path must be audible: the 2026-09-01 EDDN stall
    // retried a poison batch every 30 s for 23 hours in perfect silence
    // because ed_store's error! (and ed_eddn's idle-reconnect warn!)
    // were outside the default filter.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "ed_api=info,ed_store=info,ed_eddn=info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}
