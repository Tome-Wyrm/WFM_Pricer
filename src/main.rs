// This is a binary, not a published library — nothing here is meant to be called by code that
// needs a custom `BuildHasher`, so the generic-hasher churn this pedantic lint wants throughout
// cli.rs/mapping.rs would add noise without real benefit. Allowed crate-wide deliberately.
#![allow(clippy::implicit_hasher)]

pub mod app;
pub mod cli;
pub mod config;
mod debug_mastery;
pub mod decryption;
pub mod http;
pub mod ingestion;
pub mod logging;
pub mod mapping;
pub mod models;
pub mod pricing;
pub mod vendor;
pub mod wfm_client;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Crate-wide result alias. `Send + Sync` is required so errors can cross `.await`
/// points and be used from within `tokio::spawn`/async trait objects without every
/// call site needing to know or care which error type actually flows through.
pub type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
// Timestamped session logging: see src/logging.rs. tsprintln!/tseprintln! behave exactly
// like println!/eprintln! but also mirror into logs/session_<timestamp>.log.

/// `wfm-pricer` — Warframe.Market pricing/inventory advisor, plus the `vendor` command
/// for ranking vendor offerings by plat-efficiency.
#[derive(Parser, Debug)]
#[command(name = "wfm-pricer", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Override the inventory file path (defaults to `inventory.json`, falling back
    /// to the `AlecaFrame` `lastData.dat` location). Applies to the default pipeline
    /// only; ignored by `vendor` / `update-caches`.
    #[arg(short, long, global = true)]
    pub(crate) inventory: Option<PathBuf>,

    /// Skip listings priced below this platinum amount. Applies to the default pipeline.
    #[arg(long, global = true)]
    pub(crate) min_price: Option<f64>,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Rank vendor offerings by plat-efficiency for a location/vendor, or the whole
    /// wiki dump's match-coverage.
    Vendor {
        /// Nav-tree path (e.g. `Misc/Zariman/Cavalero`), case-insensitive. Omit for
        /// the interactive picker.
        path: Option<String>,
        /// Print the D4 WFM-match-coverage report instead of ranking anything.
        #[arg(long)]
        match_report: bool,
        /// Write the ranked table to `vendor_rankings.json` in the project root.
        #[arg(long)]
        write_json: bool,
        /// Drop offerings whose saturation ratio exceeds this value. Unset = no
        /// filtering (saturation is always shown, just not enforced).
        #[arg(long)]
        max_saturation: Option<f64>,
    },
    /// Refresh all caches (including vendor data) and exit — no inventory ingestion,
    /// no WFM login, no interactive loop. Safe to run from cron/Task Scheduler with
    /// no `.env` configured.
    UpdateCaches,
    /// Run the `--debug-mastery` checklist report (reads
    /// `config/mastery_checklist.txt`) and exit.
    DebugMastery,
    /// Display current prices of all max-rank Primed mods.
    PrimedMods {
        /// Price at minimum (unranked) instead of maximum rank.
        #[arg(long)]
        min_rank: bool,
    },
    /// Find Sets you own some but not all components of, and check whether buying the
    /// missing components off the current buy-order book would be profitable against what
    /// the completed Set currently fetches on its own buy-order book.
    CheckSets {
        /// Only show Sets whose estimated profit is at least this many platinum (can be
        /// negative to also see near-miss/losing completions). Unset = show everything that
        /// could be priced.
        #[arg(long)]
        min_profit: Option<f64>,
    },
    /// Check owned relics (by tier: Intact/Exceptional/Flawless/Radiant) against the live
    /// public buy-order book and print ready-to-send whisper messages for the best-paying
    /// buyer of each relic/tier you actually own enough of to fulfill.
    SellRelics {
        /// Only include buy orders at or above this platinum price. Unset = show every
        /// fulfillable buy order regardless of price.
        #[arg(long)]
        min_price: Option<u32>,
    },
}

#[tokio::main]
async fn main() -> AppResult<()> {
    // Must run before anything else prints: this is what populates
    // logs/session_<timestamp>.log for every tsprintln!/tsprint!/tseprintln! call below (and
    // throughout cli.rs/mapping.rs/vendor.rs/pricing.rs), with no need to pipe `cargo run`
    // through an external wrapper anymore. If this fails (e.g. read-only filesystem), we just
    // carry on without file logging rather than aborting the whole run over it.
    if let Err(e) = logging::init() {
        tseprintln!("Warning: could not initialize session log file: {e}");
    }

    dotenvy::dotenv().ok();

    // Ensure config directories exist
    std::fs::create_dir_all(config::CONFIG_DIR)?;
    std::fs::create_dir_all(config::CACHE_DIR)?;
    std::fs::create_dir_all(config::STATISTICS_DIR)?;

    let cli_args = Cli::parse();

    app::run(cli_args).await
}
