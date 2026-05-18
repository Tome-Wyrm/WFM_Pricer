pub mod models;
pub mod decryption;
pub mod ingestion;
pub mod mapping;

#[tokio::main]
async fn main() {
    println!("--- WFM Pricer Mapping Test ---");
    
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
    match mapping::map_inventory(&inventory) {
        Ok(mapped) => {
            println!("Successfully mapped {} items!", mapped.len());
            
            // Print category counts
            let mut mods = 0;
            let mut arcanes = 0;
            let mut ayatans = 0;
            let mut others = 0;
            
            for item in &mapped {
                if item.is_mod {
                    mods += 1;
                } else if item.is_arcane {
                    arcanes += 1;
                } else if item.is_ayatan {
                    ayatans += 1;
                } else {
                    others += 1;
                }
            }
            
            println!("Summary of mapped items:");
            println!("  Mods: {}", mods);
            println!("  Arcanes: {}", arcanes);
            println!("  Ayatan Sculptures / Stars: {}", ayatans);
            println!("  Prime parts, weapon parts, other: {}", others);
            
            println!("\nFirst 30 mapped items:");
            for (idx, item) in mapped.iter().take(30).enumerate() {
                println!(
                    "  {}. Name: {}, Slug: {}, Qty: {}, Rank: {}/{}",
                    idx + 1,
                    item.name,
                    item.slug,
                    item.quantity,
                    item.rank,
                    item.max_rank.map_or("N/A".to_string(), |r| r.to_string())
                );
            }
        }
        Err(e) => {
            println!("Error mapping inventory: {:?}", e);
        }
    }
}
