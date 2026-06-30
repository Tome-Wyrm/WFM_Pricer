// This is a binary, not a published library — nothing here is meant to be called by code that
// needs a custom `BuildHasher`, so the generic-hasher churn this pedantic lint wants throughout
// cli.rs/mapping.rs would add noise without real benefit. Allowed crate-wide deliberately.
#![allow(clippy::implicit_hasher)]

pub mod cli;
pub mod config;
pub mod decryption;
pub mod ingestion;
pub mod mapping;
pub mod models;
pub mod pricing;
pub mod vendor;
pub mod wfm_client;

use std::error::Error;
use std::path::{Path, PathBuf};

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
                eprintln!(
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
        eprintln!("Debug mastery checklist file not found: {checklist_path}");
        eprintln!("Please create it with one item name per line.");
        return Ok(());
    }
    let content = std::fs::read_to_string(checklist_path)?;
    let items: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

    println!("\n=== Mastery Checklist Debug (with XP details) ===");
    println!("{:<40} | {:<30} | MaxRank | Req XP | XP     | Status", "Item", "uniqueName");
    println!("{}", "-".repeat(110));

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
            println!(
                "{name:<40} | {unique:<30} | {max_rank:>3}    | {required:>6} | {xp:>6} | {status:<10} ({set_status})"
            );
        } else {
            println!("{name:<40} | Not found in WFCD/WFM");
        }
    }
    println!("=== End of Checklist ===\n");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    // Ensure config directories exist
    std::fs::create_dir_all(config::CONFIG_DIR)?;
    std::fs::create_dir_all(config::CACHE_DIR)?;
    std::fs::create_dir_all(config::STATISTICS_DIR)?;

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--vendor") {
        return vendor::run_vendor_cli().await;
    }

    println!("--- WFM Pricer System Startup ---");

    // 1. Update caches (fail fast if this doesn't work)
    mapping::update_caches().await?;

    // 2. Ingest inventory
    println!("Ingesting inventory...");
    let inventory_path = if Path::new("inventory.json").exists() {
        println!("Found inventory.json, using it directly.");
        PathBuf::from("inventory.json")
    } else {
        println!("inventory.json not found, falling back to AlecaFrame lastData.dat");
        ingestion::get_inventory_path()?
    };

    let inventory = ingestion::ingest_inventory(&inventory_path)?;

    // 3. Map inventory to WFM items
    println!("Mapping inventory items to Warframe.Market tradeable items...");
    let client = reqwest::Client::new();
    let mapped = mapping::map_inventory(&inventory, &client).await?;

    // 4. Load build maps and mastery status (needed for auto‑keep logic)
    println!("Loading build maps and mastery status...");
    let (parent_map, requirements) = mapping::load_build_maps()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, _wfm_by_slug) = mapping::load_lookup_tables()?;
    let (mastered_set, owned_built_set, frame_tier_uniques) = mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    println!("Successfully mapped {} items!", mapped.len());

    // Parse command line args for --debug-mastery
    let args: Vec<String> = std::env::args().collect();
    let debug_mastery = args.iter().any(|arg| arg == "--debug-mastery");

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
