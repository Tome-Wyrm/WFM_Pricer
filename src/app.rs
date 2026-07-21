//! The application "kernel": CLI-command dispatch and the default pipeline
//! (cache update → ingest → map → build/mastery load → optional debug-mastery
//! report → interactive CLI).
//!
//! Split out of `main.rs` so that file stays down to argument parsing, startup
//! (logging/env/config-dir setup), and handing off to [`run`] — not also owning the
//! orchestration logic itself.

use crate::{
    AppResult, Cli, Commands, cli, debug_mastery, http, ingestion, mapping, tsprintln, vendor,
};
use std::path::{Path, PathBuf};

/// Resolves the inventory file to ingest: an explicit `--inventory` override, else
/// `inventory.json` in the cwd if present, else the `AlecaFrame` `lastData.dat` fallback.
pub(crate) fn resolve_inventory_path(override_path: Option<PathBuf>) -> AppResult<PathBuf> {
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
) -> AppResult<()> {
    tsprintln!("--- WFM Pricer System Startup ---");

    // 1. Update caches (fail fast if this doesn't work)
    mapping::update_caches().await?;

    // 2. Ingest inventory
    tsprintln!("Ingesting inventory...");
    let inventory_path = resolve_inventory_path(inventory_override)?;
    let inventory = ingestion::ingest_inventory(&inventory_path)?;

    // 3. Map inventory to WFM items
    tsprintln!("Mapping inventory items to Warframe.Market tradeable items...");
    let mapped = mapping::map_inventory(&inventory, http::shared_client()).await?;

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
    .await?;

    Ok(())
}

/// Dispatches a parsed [`Cli`] invocation to the matching subcommand, or the default
/// pipeline if none was given. This is the entire application after `main` finishes
/// its startup housekeeping.
///
/// # Errors
/// Returns an error if the selected subcommand (or the default pipeline) fails —
/// see the individual `cli`/`mapping`/`vendor` functions this delegates to.
pub(crate) async fn run(cli_args: Cli) -> AppResult<()> {
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
