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
    let (wfcd_by_ref, _, _, _) = mapping::load_lookup_tables()?;
    let (mastered_set, owned_built_set) = mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

    println!("Successfully mapped {} items!", mapped.len());

    // 5. Run interactive CLI
    cli::run_cli(mapped, &parent_map, &mastered_set, &owned_built_set)
        .await
        .map_err(|e| -> Box<dyn Error> { e })?;

    Ok(())
}
