//! The `ed-api` command surface, parsed without touching the environment
//! so it can be tested.

use std::path::PathBuf;

use anyhow::{bail, Result};

pub const USAGE: &str =
    "usage: ed-api [serve | ingest | hydrate <fixture.json> | hydrate --spansh <dump.json[.gz]> \
                         | hydrate --edsm-bodies <bodies.json[.gz]> | hydrate --fdev-ids [commodity.csv] \
                         | publish-community | publish-market-daily | publish-stars \
                         | build-routing <galaxy.json[.gz]> [artifact_dir] | adopt-routing <index-dir> [artifact_dir] \
                         | reconcile-routing [artifact_dir]]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    /// The EDDN feed as its own process (`edda-eddn.service`): the same
    /// decode → writer → Postgres path `serve` ran in-process, so a
    /// serve swap no longer loses a minute of boards. Postgres is the
    /// handoff; there is no socket or queue between the two.
    Ingest,
    /// Seed from the checked-in synthetic fixture.
    HydrateFixture(PathBuf),
    /// Seed from a Spansh dump (`galaxy_populated`, `galaxy_stations` or
    /// `galaxy`, compressed or not).
    HydrateSpansh(PathBuf),
    /// Learn main-star classes from an EDSM bodies dump.
    HydrateEdsmBodies(PathBuf),
    /// Commodity display names and categories from EDCD/FDevIDs: the
    /// baked-in table by default, or a downloaded `commodity.csv`.
    HydrateFdevIds(Option<PathBuf>),
    PublishCommunity,
    /// Item 49: publish the rolling-window `market_daily` product (the
    /// boards observed in the last ~8 days; a few MB vs the full ~500).
    PublishMarketDaily,
    /// Publish the `stars` product from the stars table.
    PublishStars,
    /// Build the EDGX routing index from a galaxy dump and publish it as
    /// the `routing` product. `artifact_dir` overrides the configured one.
    BuildRouting {
        source: PathBuf,
        artifact_dir: Option<PathBuf>,
    },
    /// Adopt an index built elsewhere (the four EDGX files plus any side
    /// files) as the next routing version: the same publish as
    /// build-routing without the import — a rebase.
    AdoptRouting {
        prebuilt: PathBuf,
        artifact_dir: Option<PathBuf>,
    },
    /// Item 47: diff EDDN/EDSM knowledge against the published routing
    /// index and, when the day changed something, publish the EDGO
    /// overlay and the applied next version with the chain extended.
    ReconcileRouting { artifact_dir: Option<PathBuf> },
}

pub fn parse_command(args: Vec<String>) -> Result<Command> {
    let mut args = args.into_iter();
    let command = match args.next().as_deref().unwrap_or("serve") {
        "serve" => Command::Serve,
        "ingest" => Command::Ingest,
        "hydrate" => match (args.next(), args.next()) {
            (Some(flag), Some(path)) if flag == "--spansh" => {
                Command::HydrateSpansh(PathBuf::from(path))
            }
            (Some(flag), Some(path)) if flag == "--edsm-bodies" => {
                Command::HydrateEdsmBodies(PathBuf::from(path))
            }
            (Some(flag), path) if flag == "--fdev-ids" => {
                Command::HydrateFdevIds(path.map(PathBuf::from))
            }
            (Some(path), None) if path != "--spansh" => {
                Command::HydrateFixture(PathBuf::from(path))
            }
            _ => bail!("{USAGE}"),
        },
        "publish-community" | "publish-market" => Command::PublishCommunity,
        "publish-market-daily" => Command::PublishMarketDaily,
        "publish-stars" => Command::PublishStars,
        "build-routing" | "publish-routing" => {
            let Some(source) = args.next() else {
                bail!("{USAGE}");
            };
            Command::BuildRouting {
                source: PathBuf::from(source),
                artifact_dir: args.next().map(PathBuf::from),
            }
        }
        "adopt-routing" => {
            let Some(prebuilt) = args.next() else {
                bail!("{USAGE}");
            };
            Command::AdoptRouting {
                prebuilt: PathBuf::from(prebuilt),
                artifact_dir: args.next().map(PathBuf::from),
            }
        }
        "reconcile-routing" => Command::ReconcileRouting {
            artifact_dir: args.next().map(PathBuf::from),
        },
        command => bail!("unknown command {command:?}; {USAGE}"),
    };
    if args.next().is_some() {
        bail!("{USAGE}");
    }
    Ok(command)
}
