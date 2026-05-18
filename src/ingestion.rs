use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use crate::decryption::decrypt;
use crate::models::AlecaFrameInventory;

/// Resolves the inventory file path based on command line arguments or environment defaults.
pub fn get_inventory_path() -> Result<PathBuf, Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    
    // Check for --inventory or -i flag
    for i in 0..args.len() {
        if (args[i] == "--inventory" || args[i] == "-i") && i + 1 < args.len() {
            return Ok(PathBuf::from(&args[i + 1]));
        }
    }
    
    // Default to %LOCALAPPDATA%\AlecaFrame\lastData.dat
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let default_path = PathBuf::from(local_app_data)
            .join("AlecaFrame")
            .join("lastData.dat");
        Ok(default_path)
    } else {
        Err("LOCALAPPDATA environment variable not set, and no --inventory flag provided".into())
    }
}

/// Reads, decrypts, and extracts the AlecaFrame inventory JSON.
pub fn ingest_inventory<P: AsRef<Path>>(path: P) -> Result<AlecaFrameInventory, Box<dyn Error>> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(format!("Inventory file does not exist: {:?}", path).into());
    }

    let ciphertext = fs::read(path)?;
    
    // Try to decrypt first
    let decrypted_bytes = match decrypt(&ciphertext) {
        Ok(bytes) => bytes,
        Err(e) => {
            // Trim leading timestamp or whitespace to support already decrypted inventory.json fallback
            let mut start_idx = 0;
            while start_idx < ciphertext.len() && (ciphertext[start_idx].is_ascii_digit() || ciphertext[start_idx].is_ascii_whitespace()) {
                start_idx += 1;
            }
            if start_idx < ciphertext.len() && (ciphertext[start_idx] == b'{' || ciphertext[start_idx] == b'[') {
                ciphertext[start_idx..].to_vec()
            } else {
                return Err(format!("Decryption failed and file does not look like plain JSON: {:?}", e).into());
            }
        }
    };
    
    // Parse outer JSON
    let outer_json: serde_json::Value = serde_json::from_slice(&decrypted_bytes)
        .map_err(|e| format!("Failed to parse outer JSON: {:?}", e))?;
        
    // Extract nested InventoryJson string if present
    let inventory_value = if let Some(inventory_str) = outer_json.get("InventoryJson").and_then(|v| v.as_str()) {
        serde_json::from_str(inventory_str)
            .map_err(|e| format!("Failed to parse inner InventoryJson: {:?}", e))?
    } else {
        outer_json
    };
    
    // Deserialize into domain AlecaFrameInventory
    let inventory: AlecaFrameInventory = serde_json::from_value(inventory_value)
        .map_err(|e| format!("Failed to deserialize into AlecaFrameInventory structure: {:?}", e))?;
        
    Ok(inventory)
}
