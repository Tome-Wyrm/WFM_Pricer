pub mod cli;
pub mod config;
pub mod decryption;
pub mod ingestion;
pub mod mapping;
pub mod models;
pub mod pricing;
pub mod wfm_client;

use std::error::Error;
use std::fs;
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
    let (wfcd_by_ref, _, _, _) = mapping::load_lookup_tables()?;
    let (mastered_set, owned_built_set) = mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    println!("Successfully mapped {} items!", mapped.len());

    // Parse command line args for --debug-mastery
    let args: Vec<String> = std::env::args().collect();
    let debug_mastery = args.iter().any(|arg| arg == "--debug-mastery");

    if debug_mastery {
        // Load lookup tables to get WFCD by ref
        let (wfcd_by_ref, _, _, _) = mapping::load_lookup_tables()?;
        // Build name -> uniqueName map
        let name_to_unique: std::collections::HashMap<_, _> = wfcd_by_ref
            .values()
            .map(|item| (item.name.to_lowercase(), item.unique_name.clone()))
            .collect();

        // Read checklist file
        let checklist_path = "config/mastery_checklist.txt";
        if !Path::new(checklist_path).exists() {
            eprintln!("Debug mastery checklist file not found: {}", checklist_path);
            eprintln!("Please create it with one item name per line.");
            return Ok(());
        }
        let content = fs::read_to_string(checklist_path)?;
        let items: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

        println!("\n=== Mastery Checklist Debug ===");
        for name in items {
            let lower = name.trim().to_lowercase();
            let unique = name_to_unique.get(&lower);
            let mastered = unique.map_or(false, |u| mastered_set.contains(u));
            let status = if unique.is_none() {
                "Not found in WFCD"
            } else if mastered {
                "Mastered"
            } else {
                "Not Mastered"
            };
            println!("{:<40} | {}", name.trim(), status);
        }
        println!("=== End of Checklist ===\n");
    }

    // 5. Run interactive CLI
    cli::run_cli(mapped, &parent_map, &mastered_set, &owned_built_set)
        .await
        .map_err(|e| -> Box<dyn Error> { e })?;

    Ok(())
}
