// This is a binary, not a published library — nothing here is meant to be called by code that
// needs a custom `BuildHasher`, so the generic-hasher churn this pedantic lint wants throughout
// cli.rs/mapping.rs would add noise without real benefit. Allowed crate-wide deliberately.
#![allow(clippy::implicit_hasher)]

pub mod cli;
pub mod config;
mod debug_mastery;
pub mod decryption;
pub mod ingestion;
pub mod logging;
pub mod mapping;
pub mod models;
pub mod pricing;
pub mod vendor;
pub mod wfm_client;

use clap::{Parser, Subcommand};
use std::error::Error;
use std::path::{Path, PathBuf};
// Timestamped session logging: see src/logging.rs. tsprintln!/tseprintln! behave exactly
// like println!/eprintln! but also mirror into logs/session_<timestamp>.log.

/// `wfm-pricer` — Warframe.Market pricing/inventory advisor, plus the `vendor` command
/// for ranking vendor offerings by plat-efficiency.
#[derive(Parser, Debug)]
#[command(name = "wfm-pricer", version, about, long_about = None)]
struct Cli {
    /// Override the inventory file path (defaults to `inventory.json`, falling back
    /// to the `AlecaFrame` `lastData.dat` location). Applies to the default pipeline
    /// only; ignored by `vendor` / `update-caches`.
    #[arg(short, long, global = true)]
    inventory: Option<PathBuf>,

    /// Skip listings priced below this platinum amount. Applies to the default pipeline.
    #[arg(long, global = true)]
    min_price: Option<f64>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
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

/// Resolves the inventory file to ingest: an explicit `--inventory` override, else
/// `inventory.json` in the cwd if present, else the `AlecaFrame` `lastData.dat` fallback.
pub(crate) fn resolve_inventory_path(
    override_path: Option<PathBuf>,
) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(p) = override_path {
        tsprintln!("Using --inventory override: {}", p.display());
        return Ok(p);
    }
    if Path::new("inventory.json").exists() {
        tsprintln!("Found inventory.json, using it directly.");
        return Ok(PathBuf::from("inventory.json"));
    }
    tsprintln!("inventory.json not found, falling back to AlecaFrame lastData.dat");
    ingestion::get_inventory_path()
}

/// Runs today's full default pipeline (cache update → ingest → map → build/mastery
/// load → optional debug-mastery report → interactive CLI). Unchanged behavior from
/// before the Phase G clap migration, aside from `inventory_override` replacing the
/// old `inventory.json`-or-bust check.
async fn run_default_pipeline(
    inventory_override: Option<PathBuf>,
    debug_mastery: bool,
    min_price: Option<f64>,
) -> Result<(), Box<dyn Error>> {
    tsprintln!("--- WFM Pricer System Startup ---");

    // 1. Update caches (fail fast if this doesn't work)
    mapping::update_caches().await?;

    // 2. Ingest inventory
    tsprintln!("Ingesting inventory...");
    let inventory_path = resolve_inventory_path(inventory_override)?;
    let inventory = ingestion::ingest_inventory(&inventory_path)?;

    // 3. Map inventory to WFM items
    tsprintln!("Mapping inventory items to Warframe.Market tradeable items...");
    let client = reqwest::Client::new();
    let mapped = mapping::map_inventory(&inventory, &client).await?;

    // 4. Load build maps and mastery status (needed for auto‑keep logic)
    tsprintln!("Loading build maps and mastery status...");
    let (parent_map, requirements) = mapping::load_build_maps()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, _wfm_by_slug) = mapping::load_lookup_tables()?;
    let (mastered_set, owned_built_set, frame_tier_uniques) =
        mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    tsprintln!("Successfully mapped {} items!", mapped.len());

    if debug_mastery {
        debug_mastery::run_debug_mastery_checklist(
            &inventory,
            &wfcd_by_ref,
            &wfm_by_ref,
            &wfm_by_name,
            &mastered_set,
            &frame_tier_uniques,
        )?;
    }

    // 5. Run interactive CLI
    cli::run_cli(
        mapped,
        &parent_map,
        &mastered_set,
        &owned_built_set,
        &requirements,
        &wfcd_by_ref,
        &wfm_by_name,
        min_price,
    )
    .await
    .map_err(|e| e as Box<dyn Error>)?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
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

    match cli_args.command {
        Some(Commands::Vendor {
            path,
            match_report,
            write_json,
            max_saturation,
        }) => {
            vendor::run_vendor_cli(path.as_deref(), match_report, write_json, max_saturation).await
        }
        Some(Commands::UpdateCaches) => mapping::update_caches().await,
        Some(Commands::PrimedMods { min_rank }) => cli::run_primed_mod_prices(min_rank).await,
        Some(Commands::CheckSets { min_profit }) => cli::run_check_sets_cli(min_profit).await,
        Some(Commands::SellRelics { min_price }) => cli::run_sell_relics_cli(min_price).await,
        Some(Commands::DebugMastery) => {
            run_default_pipeline(cli_args.inventory, true, cli_args.min_price).await
        }
        None => run_default_pipeline(cli_args.inventory, false, cli_args.min_price).await,
    }
}
