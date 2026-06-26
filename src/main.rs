pub mod cli;
pub mod config;
pub mod decryption;
pub mod ingestion;
pub mod mapping;
pub mod models;
pub mod pricing;
pub mod wfm_client;

use std::error::Error;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
fn is_eligible_for_mastery_checklist(unique_name: &str) -> bool {
    !unique_name.starts_with("SolNode")
        && !unique_name.contains("/StoreItems/")
        && !unique_name.contains("PvPVariant")
        && !unique_name.contains("RewardItem")
        && !unique_name.contains("/Emotes/")
        && !unique_name.contains("Doppelganger") // exclude the fake Grimoire
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    // Ensure config directories exist
    std::fs::create_dir_all(config::CONFIG_DIR)?;
    std::fs::create_dir_all(config::CACHE_DIR)?;
    std::fs::create_dir_all(config::STATISTICS_DIR)?;

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
    let (parent_map, _requirements) = mapping::load_build_maps()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, _wfm_by_slug) = mapping::load_lookup_tables()?;
    let (mastered_set, owned_built_set) = mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    println!("Successfully mapped {} items!", mapped.len());

    // Parse command line args for --debug-mastery
    let args: Vec<String> = std::env::args().collect();
    let debug_mastery = args.iter().any(|arg| arg == "--debug-mastery");

    if debug_mastery {
        // Filter decoys
        fn is_eligible_for_mastery_checklist(unique_name: &str) -> bool {
            !unique_name.starts_with("SolNode")
                && !unique_name.contains("/StoreItems/")
                && !unique_name.contains("PvPVariant")
                && !unique_name.contains("RewardItem")
                && !unique_name.contains("/Emotes/")  // fixes Gaze collision
        }

        // ---- Build XP map (override MechSuits) ----
        let mut xp_map = std::collections::HashMap::new();
        if let Some(xp_info) = inventory.get("XPInfo").and_then(|v| v.as_array()) {
            for entry in xp_info {
                if let (Some(unique), Some(xp)) = (
                    entry.get("ItemType").and_then(|v| v.as_str()),
                    entry.get("XP").and_then(|v| v.as_u64()),
                ) {
                    xp_map.insert(unique.to_string(), xp);
                }
            }
        }
        if let Some(mech_suits) = inventory.get("MechSuits").and_then(|v| v.as_array()) {
            for entry in mech_suits {
                if let (Some(unique), Some(xp)) = (
                    entry.get("ItemType").and_then(|v| v.as_str()),
                    entry.get("XP").and_then(|v| v.as_u64()),
                ) {
                    xp_map.insert(unique.to_string(), xp);
                }
            }
        }

        // ---- Build name->unique map (filtered) ----
        let mut name_to_unique = std::collections::HashMap::new();
        for (unique, item) in &wfcd_by_ref {
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
        for (name, item) in &wfm_by_name {
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
            eprintln!("Debug mastery checklist file not found: {}", checklist_path);
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
                    .map(|w| w.name.as_str())
                    .unwrap_or("");

                // ---- CLASSIFICATION ----
                let is_overlevel = display_name.starts_with("Kuva ")
                    || display_name.starts_with("Tenet ")
                    || display_name.starts_with("Coda ")
                    || display_name == "Paracesis"
                    || unique.contains("EntratiMech");

                let is_frame_tier = unique.contains("/Powersuits/")          // Warframes, Archwing suits
                    || unique.contains("/SentinelPowersuits/")              // Sentinel bodies (not weapons)
                    || (unique.contains("/MoaPets/") && !unique.contains("/MoaPetComponents/"))  // MOA bodies (not weapons)
                    || (unique.contains("/Hounds/") && !unique.contains("/ZanukaPetMeleeWeapon/")) // Hound bodies (not weapons)
                    || unique.contains("/CatbrowPet/")                      // Beast companions
                    || unique.contains("/CreaturePets/")                    // Predasites, Vulpaphylas
                    || unique.contains("/KubrowPets/")                      // Kubrow bodies
                    || unique.contains("/Hoverboard/")                      // K‑Drives
                    || unique.contains("/EntratiMech/");                    // Necramech bodies

                let required = if is_overlevel {
                    if unique.contains("EntratiMech") {
                        1_600_000
                    } else {
                        800_000
                    }
                } else if is_frame_tier {
                    900_000
                } else {
                    450_000
                };

                let max_rank = if let Some(wfm_item) = wfm_by_ref.get(&unique) {
                    wfm_item.max_rank.unwrap_or(30)
                } else if let Some(wfcd) = wfcd_by_ref.get(&unique) {
                    wfcd.fusion_limit
                        .or_else(|| wfcd.level_stats.as_ref().map(|v| v.len() as u32 - 1))
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
                    "{:<40} | {:<30} | {:>3}    | {:>6} | {:>6} | {:<10} ({})",
                    name, unique, max_rank, required, xp, status, set_status
                );
            } else {
                println!("{:<40} | Not found in WFCD/WFM", name);
            }
        }
        println!("=== End of Checklist ===\n");
    }

    // 5. Run interactive CLI
    cli::run_cli(mapped, &parent_map, &mastered_set, &owned_built_set)
        .await
        .map_err(|e| -> Box<dyn Error> { e })?;

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
}
