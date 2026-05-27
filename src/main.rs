pub mod cli;
pub mod client;
pub mod config;
pub mod decryption;
pub mod ingestion;
pub mod mapping;
pub mod models;
pub mod pricing;

use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() {
    // Load .env file
    dotenvy::dotenv().ok();
        std::fs::create_dir_all(config::CONFIG_DIR).expect("Failed to create config dir");
        std::fs::create_dir_all(config::CACHE_DIR).expect("Failed to create cache dir");
        std::fs::create_dir_all(config::STATISTICS_DIR).expect("Failed to create statistics dir");

    println!("--- WFM Pricer System Startup ---");

    // 1. Update caches
    if let Err(e) = mapping::update_caches().await {
        println!("Error updating caches: {e:?}");
        return;
    }

    // 2. Ingest inventory
    println!("Ingesting inventory...");

    // Determine inventory source: use inventory.json if present, else fallback to AlecaFrame path
    let inventory_path = if Path::new("inventory.json").exists() {
        println!("Found inventory.json, using it directly.");
        PathBuf::from("inventory.json")
    } else {
        println!("inventory.json not found, falling back to AlecaFrame lastData.dat");
        match ingestion::get_inventory_path() {
            Ok(p) => p,
            Err(e) => {
                println!("Could not determine AlecaFrame inventory file location: {e}");
                return;
            }
        }
    };

    let inventory = match ingestion::ingest_inventory(&inventory_path) {
        Ok(inv) => inv,
        Err(e) => {
            println!("Error ingesting inventory from {}: {e:?}", inventory_path.display());
            return;
        }
    };

    // 3. Map inventory
    println!("Mapping inventory items to Warframe.Market tradeable items...");
    let mapped = match mapping::map_inventory(&inventory) {
        Ok(mapped) => mapped,
        Err(e) => {
            println!("Error mapping inventory: {e:?}");
            return;
        }
    };

    println!("Successfully mapped {} items!", mapped.len());

    // 4. Run interactive CLI
    if let Err(e) = cli::run_cli(mapped).await {
        println!("CLI execution failed: {e:?}");
    }
}
