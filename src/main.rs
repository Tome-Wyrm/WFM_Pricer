// This is a binary, not a published library — nothing here is meant to be called by code that
// needs a custom `BuildHasher`, so the generic-hasher churn this pedantic lint wants throughout
// cli.rs/mapping.rs would add noise without real benefit. Allowed crate-wide deliberately.
#![allow(clippy::implicit_hasher)]

pub mod cli;
pub mod config;
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
    /// to the AlecaFrame `lastData.dat` location). Applies to the default pipeline
    /// only; ignored by `vendor` / `update-caches`.
    #[arg(short, long, global = true)]
    inventory: Option<PathBuf>,

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
}

fn is_eligible_for_mastery_checklist(unique_name: &str) -> bool {
    !unique_name.starts_with("SolNode")
        && !unique_name.contains("/StoreItems/")
        && !unique_name.contains("PvPVariant")
        && !unique_name.contains("RewardItem")
        && !unique_name.contains("/Emotes/")
        && !unique_name.contains("Doppelganger") // exclude the fake Grimoire
}

/// Runs the `--debug-mastery` checklist report: reads `config/mastery_checklist.txt` and prints
/// each listed item's resolved `uniqueName`, XP, mastery threshold, and Mastered/Not Mastered
/// status, for spot-checking the mastery logic against a real account.
///
/// Pulled out of `main` purely to keep `main` itself short; no behavior change.
///
/// # Errors
/// Returns an error if the checklist file exists but cannot be read.
#[allow(clippy::too_many_lines)]
fn run_debug_mastery_checklist(
    inventory: &serde_json::Value,
    wfcd_by_ref: &std::collections::HashMap<String, crate::models::WfcdItem>,
    wfm_by_ref: &std::collections::HashMap<String, crate::models::WfmItem>,
    wfm_by_name: &std::collections::HashMap<String, crate::models::WfmItem>,
    mastered_set: &std::collections::HashSet<String>,
    frame_tier_uniques: &std::collections::HashSet<String>,
) -> Result<(), Box<dyn Error>> {
    // ---- Build XP map ----
    // XPInfo is the only reliable source for the reasons documented on
    // load_mastery_and_ownership — do not reintroduce a MechSuits/SpaceGuns-style override here.
    let mut xp_map = std::collections::HashMap::new();
    if let Some(xp_info) = inventory.get("XPInfo").and_then(|v| v.as_array()) {
        for entry in xp_info {
            if let (Some(unique), Some(xp)) = (
                entry.get("ItemType").and_then(|v| v.as_str()),
                entry.get("XP").and_then(serde_json::Value::as_u64),
            ) {
                xp_map.insert(unique.to_string(), xp);
            }
        }
    }

    // ---- Build name->unique map (filtered) ----
    let mut name_to_unique = std::collections::HashMap::new();
    for (unique, item) in wfcd_by_ref {
        if is_eligible_for_mastery_checklist(unique) {
            let norm = item.name.to_lowercase();
            if let Some(existing) = name_to_unique.get(&norm) {
                tseprintln!(
                    "WARNING: Ambiguous display name '{}' maps to both '{}' and '{}' — picking first.",
                    item.name, existing, unique
                );
            } else {
                name_to_unique.insert(norm, unique.clone());
            }
        }
    }
    // WFM fallback (also filtered)
    for (name, item) in wfm_by_name {
        let norm = name.to_lowercase();
        if let Some(gr) = &item.game_ref
            && let Some(wfcd_item) = wfcd_by_ref.get(gr)
                && is_eligible_for_mastery_checklist(&wfcd_item.unique_name)
            {
                name_to_unique.entry(norm).or_insert(wfcd_item.unique_name.clone());
            }
    }

    // ---- Read checklist ----
    let checklist_path = "config/mastery_checklist.txt";
    if !std::path::Path::new(checklist_path).exists() {
        tseprintln!("Debug mastery checklist file not found: {checklist_path}");
        tseprintln!("Please create it with one item name per line.");
        return Ok(());
    }
    let content = std::fs::read_to_string(checklist_path)?;
    let items: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

    tsprintln!("\n=== Mastery Checklist Debug (with XP details) ===");
    tsprintln!("{:<40} | {:<30} | MaxRank | Req XP | XP     | Status", "Item", "uniqueName");
    tsprintln!("{}", "-".repeat(110));

    for name in items {
        let name = name.trim();
        let norm = name.to_lowercase();

        let unique = name_to_unique.get(&norm).cloned()
            .or_else(|| {
                if norm.contains('&') {
                    let alt = norm.replace('&', "and");
                    name_to_unique.get(&alt).cloned()
                } else {
                    None
                }
            });

        if let Some(unique) = unique {
            let display_name = wfcd_by_ref.get(&unique)
                .map_or("", |w| w.name.as_str());

            // Same classification + threshold logic the production mastery pass uses, so the
            // debug print can never disagree with the real keep/sell decisions.
            let required = mapping::mastery_threshold(display_name, &unique, frame_tier_uniques.contains(&unique));

            let max_rank = if let Some(wfm_item) = wfm_by_ref.get(&unique) {
                wfm_item.max_rank.unwrap_or(30)
            } else if let Some(wfcd) = wfcd_by_ref.get(&unique) {
                wfcd.fusion_limit
                    .or_else(|| wfcd.level_stats.as_ref().map(|v| u32::try_from(v.len().saturating_sub(1)).unwrap_or(30)))
                    .unwrap_or(30)
            } else {
                30
            };

            let xp = xp_map.get(&unique).copied().unwrap_or(0);
            let status = if xp == 0 {
                "No XP record"
            } else if xp >= required {
                "Mastered"
            } else {
                "Not Mastered"
            };
            let set_status = if mastered_set.contains(&unique) { "in set" } else { "not in set" };
            tsprintln!(
                "{name:<40} | {unique:<30} | {max_rank:>3}    | {required:>6} | {xp:>6} | {status:<10} ({set_status})"
            );
        } else {
            tsprintln!("{name:<40} | Not found in WFCD/WFM");
        }
    }
    tsprintln!("=== End of Checklist ===\n");
    Ok(())
}

/// Resolves the inventory file to ingest: an explicit `--inventory` override, else
/// `inventory.json` in the cwd if present, else the AlecaFrame `lastData.dat` fallback.
pub(crate) fn resolve_inventory_path(override_path: Option<PathBuf>) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(p) = override_path {
        tsprintln!("Using --inventory override: {}", p.display());
        return Ok(p);
    }
    if Path::new("inventory.json").exists() {
        tsprintln!("Found inventory.json, using it directly.");
        return Ok(PathBuf::from("inventory.json"));
    }
    tsprintln!("inventory.json not found, falling back to AlecaFrame lastData.dat");
    Ok(ingestion::get_inventory_path()?)
}

/// Runs today's full default pipeline (cache update → ingest → map → build/mastery
/// load → optional debug-mastery report → interactive CLI). Unchanged behavior from
/// before the Phase G clap migration, aside from `inventory_override` replacing the
/// old `inventory.json`-or-bust check.
async fn run_default_pipeline(inventory_override: Option<PathBuf>, debug_mastery: bool) -> Result<(), Box<dyn Error>> {
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
    let (mastered_set, owned_built_set, frame_tier_uniques) = mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    tsprintln!("Successfully mapped {} items!", mapped.len());

    if debug_mastery {
        run_debug_mastery_checklist(&inventory, &wfcd_by_ref, &wfm_by_ref, &wfm_by_name, &mastered_set, &frame_tier_uniques)?;
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
        Some(Commands::Vendor { path, match_report, write_json, max_saturation }) => {
            vendor::run_vendor_cli(path.as_deref(), match_report, write_json, max_saturation).await
        }
        Some(Commands::UpdateCaches) => mapping::update_caches().await,
        Some(Commands::PrimedMods { min_rank }) => cli::run_primed_mod_prices(min_rank).await,
        Some(Commands::CheckSets { min_profit }) => cli::run_check_sets_cli(min_profit).await,
        Some(Commands::DebugMastery) => run_default_pipeline(cli_args.inventory, true).await,
        None => run_default_pipeline(cli_args.inventory, false).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoy_entries_are_excluded_from_checklist_matching() {
        assert!(!is_eligible_for_mastery_checklist("SolNode105"));
        assert!(!is_eligible_for_mastery_checklist("/Lotus/Types/StoreItems/SuitCustomizations/ColourPickerJadeItem"));
        assert!(!is_eligible_for_mastery_checklist("/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/PvPVariantTipOne"));
        assert!(!is_eligible_for_mastery_checklist("/Lotus/Types/Items/Deimos/WoundedInfestedPredatorUncommonRewardItem"));
        assert!(is_eligible_for_mastery_checklist("/Lotus/Powersuits/Fairy/Fairy"));
    }

    #[test]
    fn doppelganger_decoy_is_excluded() {
        // Previously only excluded by the dead, test-only copy of this function — the nested
        // copy actually used by --debug-mastery was missing this check. Adjust the fixture
        // string below to a real Doppelganger uniqueName if you have one on hand.
        assert!(!is_eligible_for_mastery_checklist("/Lotus/Types/Game/PlayerCustomizations/Doppelganger/SomeFakeGrimoire"));
    }
}
