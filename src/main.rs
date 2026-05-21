pub mod cli;
pub mod client;
pub mod config;
pub mod decryption;
pub mod ingestion;
pub mod mapping;
pub mod models;
pub mod pricing;


#[tokio::main]
async fn main() {
    // Load .env file
    dotenvy::dotenv().ok();
        std::fs::create_dir_all(config::CONFIG_DIR)?;
        std::fs::create_dir_all(config::CACHE_DIR)?;
        std::fs::create_dir_all(config::STATISTICS_DIR)?;

    println!("--- WFM Pricer System Startup ---");
    
    // 1. Update caches
    if let Err(e) = mapping::update_caches().await {
        println!("Error updating caches: {e:?}");
        return;
    }
    
    // 2. Ingest inventory
    println!("Ingesting inventory...");
    let inventory = match ingestion::ingest_inventory("inventory.json") {
        Ok(inv) => inv,
        Err(e) => {
            println!("Error ingesting inventory: {e:?}");
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
