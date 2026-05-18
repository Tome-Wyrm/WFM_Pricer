pub mod models;
pub mod decryption;
pub mod ingestion;

fn main() {
    println!("Loading inventory.json...");
    match ingestion::ingest_inventory("inventory.json") {
        Ok(inventory) => {
            println!("Successfully parsed inventory!");
            if let Some(ref raw) = inventory.raw_upgrades {
                println!("Found {} raw upgrades", raw.len());
            }
            if let Some(ref upg) = inventory.upgrades {
                println!("Found {} upgrades", upg.len());
            }
            if let Some(ref misc) = inventory.misc_items {
                println!("Found {} misc items", misc.len());
            }
            if let Some(ref fusion) = inventory.fusion_treasures {
                println!("Found {} fusion treasures (Ayatan sculptures)", fusion.len());
            }
        }
        Err(e) => {
            println!("Error parsing inventory: {:?}", e);
        }
    }
}
