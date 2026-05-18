pub mod models;
pub mod decryption;
pub mod ingestion;
pub mod mapping;
pub mod pricing;
pub mod client;
pub mod cli;

#[tokio::main]
async fn main() {
    // Load .env file
    dotenvy::dotenv().ok();

    println!("--- WFM Pricer System Startup ---");
    
    // 1. Update caches
    if let Err(e) = mapping::update_caches().await {
        println!("Error updating caches: {:?}", e);
        return;
    }
    
    // 2. Ingest inventory
    println!("Ingesting inventory...");
    let inventory = match ingestion::ingest_inventory("inventory.json") {
        Ok(inv) => inv,
        Err(e) => {
            println!("Error ingesting inventory: {:?}", e);
            return;
        }
    };
    
    // 3. Map inventory
    println!("Mapping inventory items to Warframe.Market tradeable items...");
    let mapped = match mapping::map_inventory(&inventory) {
        Ok(mapped) => mapped,
        Err(e) => {
            println!("Error mapping inventory: {:?}", e);
            return;
        }
    };

    println!("Successfully mapped {} items!", mapped.len());
    
    // 4. Run interactive CLI
    if let Err(e) = cli::run_cli(mapped).await {
        println!("CLI execution failed: {:?}", e);
    }
}
