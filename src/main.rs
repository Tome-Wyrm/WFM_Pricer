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
    let (wfcd_by_ref, _wfm_by_ref, wfm_by_name, _wfm_by_slug) = mapping::load_lookup_tables()?;
    let (mastered_set, owned_built_set) = mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    println!("Successfully mapped {} items!", mapped.len());

    // Parse command line args for --debug-mastery
    let args: Vec<String> = std::env::args().collect();
    let debug_mastery = args.iter().any(|arg| arg == "--debug-mastery");

    if debug_mastery {
        // Build a map of uniqueName -> XP from the inventory's XPInfo
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

        // Build a map of exact normalized WFCD name -> uniqueName.
        let mut name_to_unique = std::collections::HashMap::new();
        for (unique, item) in &wfcd_by_ref {
            let norm = item.name.to_lowercase();
            name_to_unique.insert(norm, unique.clone());
        }
        // Also add WFM display names as fallback (they match WFCD, but just in case)
        for (name, item) in &wfm_by_name {
            let norm = name.to_lowercase();
            if let Some(gr) = &item.game_ref {
                if let Some(wfcd_item) = wfcd_by_ref.get(gr) {
                    name_to_unique.entry(norm).or_insert(wfcd_item.unique_name.clone());
                }
            }
        }

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

            // Try exact match first.
            let unique = name_to_unique.get(&norm).cloned()
                // Only special case: WFCD uses "and" instead of "&" (e.g., "Sirius & Orion").
                .or_else(|| {
                    if norm.contains('&') {
                        let alt = norm.replace('&', "and");
                        name_to_unique.get(&alt).cloned()
                    } else {
                        None
                    }
                });

            if let Some(unique) = unique {
                let max_rank = wfcd_by_ref.get(&unique)
                    .and_then(|w| {
                        w.fusion_limit
                            .or_else(|| w.level_stats.as_ref().map(|v| v.len() as u32 - 1))
                    })
                    .unwrap_or(30);
                let required = max_rank * 15000;
                let xp = xp_map.get(&unique).copied().unwrap_or(0);
                let status = if xp == 0 {
                    "No XP record"
                } else if xp >= required as u64 {
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
