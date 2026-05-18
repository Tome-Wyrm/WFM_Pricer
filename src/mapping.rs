use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use reqwest::header::USER_AGENT;

use crate::models::{
    AlecaFrameInventory, MappedItem, WfcdItem, WfmItemFlat, WfmV1ItemsResponse
};

pub const CACHE_DIR: &str = "cache";
pub const METADATA_FILE: &str = "cache/cache_metadata.json";
pub const WFCD_CACHE_FILE: &str = "cache/wfcd_all_cache.json";
pub const WFM_CACHE_FILE: &str = "cache/wfm_items_cache.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub wfcd_commit_hash: String,
    pub last_updated: String,
}

#[derive(Debug, Clone)]
pub struct AyatanStaticDef {
    pub name: &'static str,
    pub game_ref: &'static str,
    pub slug: &'static str,
    pub empty_endo: u32,
    pub filled_endo: u32,
    pub fully_filled_mask: u32,
}

pub const AYATANS: &[AyatanStaticDef] = &[
    AyatanStaticDef {
        name: "Ayatan Sah Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexA",
        slug: "ayatan_sah_sculpture",
        empty_endo: 300,
        filled_endo: 1500,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Ayr Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexB",
        slug: "ayatan_ayr_sculpture",
        empty_endo: 325,
        filled_endo: 1425,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Orta Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexC",
        slug: "ayatan_orta_sculpture",
        empty_endo: 650,
        filled_endo: 2700,
        fully_filled_mask: 15,
    },
    AyatanStaticDef {
        name: "Ayatan Vaya Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexD",
        slug: "ayatan_vaya_sculpture",
        empty_endo: 400,
        filled_endo: 1800,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Piv Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexE",
        slug: "ayatan_piv_sculpture",
        empty_endo: 375,
        filled_endo: 1725,
        fully_filled_mask: 31,
    },
    AyatanStaticDef {
        name: "Ayatan Anasa Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexF",
        slug: "ayatan_anasa_sculpture",
        empty_endo: 2000,
        filled_endo: 3450,
        fully_filled_mask: 15,
    },
    AyatanStaticDef {
        name: "Ayatan Valana Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexG",
        slug: "ayatan_valana_sculpture",
        empty_endo: 325,
        filled_endo: 1575,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Hemakara Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexJ",
        slug: "ayatan_hemakara_sculpture",
        empty_endo: 350,
        filled_endo: 2600,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Zambuka Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexK",
        slug: "ayatan_zambuka_sculpture",
        empty_endo: 450,
        filled_endo: 2600,
        fully_filled_mask: 31,
    },
];

// Ayatan Stars definitions
pub const CYAN_STAR_REF: &str = "/Lotus/Types/Items/FusionTreasures/OroFusexOrnamentA";
pub const AMBER_STAR_REF: &str = "/Lotus/Types/Items/FusionTreasures/OroFusexOrnamentB";

/// Ensures all necessary cache data is fetched, updated, and verified.
pub async fn update_caches() -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(CACHE_DIR)?;

    // 1. Get latest WFCD master commit SHA
    let client = reqwest::Client::new();
    println!("Checking latest WFCD commit hash...");
    let response = client
        .get("https://api.github.com/repos/WFCD/warframe-items/commits/master")
        .header(USER_AGENT, "wfm-pricer-cli")
        .send()
        .await?;
        
    if !response.status().is_success() {
        return Err(format!("Failed to fetch WFCD commit hash: {}", response.status()).into());
    }

    let commit_info: serde_json::Value = response.json().await?;
    let latest_sha = commit_info["sha"]
        .as_str()
        .ok_or("Could not parse commit sha from GitHub response")?
        .to_string();

    println!("Latest WFCD Commit SHA: {}", latest_sha);

    // 2. Check if cache is still valid
    let mut cache_invalidated = true;
    if Path::new(METADATA_FILE).exists()
        && Path::new(WFCD_CACHE_FILE).exists()
        && Path::new(WFM_CACHE_FILE).exists()
    {
        if let Ok(metadata_str) = fs::read_to_string(METADATA_FILE) {
            if let Ok(metadata) = serde_json::from_str::<CacheMetadata>(&metadata_str) {
                if metadata.wfcd_commit_hash == latest_sha {
                    cache_invalidated = false;
                    println!("Cache is up to date (SHA matches).");
                }
            }
        }
    }

    // 3. If invalidated, re-fetch both
    if cache_invalidated {
        println!("Cache is missing or stale. Re-fetching data...");
        
        // Fetch WFCD All.json
        println!("Fetching WFCD All.json...");
        let wfcd_resp = client
            .get("https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/All.json")
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await?;
            
        if !wfcd_resp.status().is_success() {
            return Err(format!("Failed to fetch All.json: {}", wfcd_resp.status()).into());
        }
        
        let all_json_bytes = wfcd_resp.bytes().await?;
        fs::write(WFCD_CACHE_FILE, all_json_bytes)?;
        println!("WFCD All.json cached successfully.");

        // Fetch WFM v1 items list
        println!("Fetching WFM v1 items list...");
        let wfm_resp_result = client
            .get("https://api.warframe.market/v1/items")
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await;
            
        let wfm_bytes = match wfm_resp_result {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await?;
                bytes.to_vec()
            }
            _ => {
                println!("WFM API request failed or was blocked by Cloudflare. Attempting to use local v2_items.json fallback...");
                if std::path::Path::new("v2_items.json").exists() {
                    let v2_str = fs::read_to_string("v2_items.json")?;
                    
                    #[derive(Debug, Deserialize)]
                    struct V2Item {
                        id: String,
                        slug: String,
                        i18n: V2I18n,
                    }
                    #[derive(Debug, Deserialize)]
                    struct V2I18n {
                        en: V2En,
                    }
                    #[derive(Debug, Deserialize)]
                    struct V2En {
                        name: String,
                    }
                    #[derive(Debug, Deserialize)]
                    struct V2Root {
                        data: Vec<V2Item>,
                    }
                    
                    let v2_root: V2Root = serde_json::from_str(&v2_str)
                        .map_err(|e| format!("Failed to parse local v2_items.json fallback: {:?}", e))?;
                        
                    let flat_items: Vec<WfmItemFlat> = v2_root.data.into_iter().map(|item| WfmItemFlat {
                        id: item.id,
                        url_name: item.slug,
                        item_name: item.i18n.en.name,
                    }).collect();
                    
                    let response_payload = WfmV1ItemsResponse {
                        payload: crate::models::WfmV1ItemsPayload {
                            items: flat_items,
                        }
                    };
                    
                    serde_json::to_vec(&response_payload)?
                } else {
                    return Err("WFM items API request failed, and local v2_items.json fallback file is missing".into());
                }
            }
        };
        
        fs::write(WFM_CACHE_FILE, wfm_bytes)?;
        println!("WFM items list cached successfully.");

        // Update metadata
        let metadata = CacheMetadata {
            wfcd_commit_hash: latest_sha,
            last_updated: format!("{:?}", std::time::SystemTime::now()),
        };
        let metadata_str = serde_json::to_string_pretty(&metadata)?;
        fs::write(METADATA_FILE, metadata_str)?;
        println!("Cache metadata updated.");
    }

    Ok(())
}

/// Helper function to perform exact and suffix-stripped matching of a name against WFM items.
fn find_wfm_match<'a>(
    name: &str,
    wfm_by_name: &'a HashMap<String, &'a WfmItemFlat>
) -> Option<&'a WfmItemFlat> {
    let lower_name = name.to_lowercase();
    
    // 1. Exact match (case-insensitive)
    if let Some(item) = wfm_by_name.get(&lower_name) {
        return Some(*item);
    }
    
    // 2. Trailing ' Set' fallback
    if lower_name.ends_with(" set") {
        let stripped = &lower_name[..lower_name.len() - 4];
        if let Some(item) = wfm_by_name.get(stripped) {
            return Some(*item);
        }
    }
    
    None
}

/// Performs item intersection and maps raw inventory to WFM tradeable items.
pub fn map_inventory(inventory: &AlecaFrameInventory) -> Result<Vec<MappedItem>, Box<dyn Error>> {
    // 1. Load caches
    if !Path::new(WFCD_CACHE_FILE).exists() || !Path::new(WFM_CACHE_FILE).exists() {
        return Err("Cache files missing. Please run update_caches first.".into());
    }

    println!("Loading caches from disk for mapping...");
    let wfcd_str = fs::read_to_string(WFCD_CACHE_FILE)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&wfcd_str)
        .map_err(|e| format!("Failed to parse cached WFCD All.json: {:?}", e))?;

    let wfm_str = fs::read_to_string(WFM_CACHE_FILE)?;
    let wfm_response: WfmV1ItemsResponse = serde_json::from_str(&wfm_str)
        .map_err(|e| format!("Failed to parse cached WFM items list: {:?}", e))?;

    // Create lookup tables
    let mut wfcd_by_ref = HashMap::new();
    for item in &wfcd_items {
        wfcd_by_ref.insert(item.unique_name.clone(), item);
    }

    let mut wfm_by_name = HashMap::new();
    for item in &wfm_response.payload.items {
        wfm_by_name.insert(item.item_name.to_lowercase(), item);
    }

    let mut mapped_results = Vec::new();

    // Helper closure to map a single game_ref and quantity/rank combination
    let map_single = |game_ref: &str, qty: u32, rank: u32, sockets: Option<u32>| -> Option<MappedItem> {
        // A. Check static Ayatans and Stars first
        if game_ref == CYAN_STAR_REF {
            return Some(MappedItem {
                id: "58ca59c071d7d022b7405e32".to_string(), // WFM id for Cyan Star
                slug: "ayatan_cyan_star".to_string(),
                name: "Ayatan Cyan Star".to_string(),
                quantity: qty,
                rank: 0,
                max_rank: None,
                is_mod: false,
                is_arcane: false,
                is_ayatan: true,
                game_ref: game_ref.to_string(),
            });
        }
        
        if game_ref == AMBER_STAR_REF {
            return Some(MappedItem {
                id: "58ca5a1b71d7d022b7405e35".to_string(), // WFM id for Amber Star
                slug: "ayatan_amber_star".to_string(),
                name: "Ayatan Amber Star".to_string(),
                quantity: qty,
                rank: 0,
                max_rank: None,
                is_mod: false,
                is_arcane: false,
                is_ayatan: true,
                game_ref: game_ref.to_string(),
            });
        }

        if let Some(def) = AYATANS.iter().find(|a| a.game_ref == game_ref) {
            let is_filled = sockets.unwrap_or(0) == def.fully_filled_mask;
            // WFM sells either unfilled or filled, but wait: WFM items list only has "Ayatan Orta Sculpture" slug.
            // We'll map to the standard sculpture slug and name.
            if let Some(wfm_item) = wfm_by_name.get(&def.name.to_lowercase()) {
                return Some(MappedItem {
                    id: wfm_item.id.clone(),
                    slug: wfm_item.url_name.clone(),
                    name: wfm_item.item_name.clone(),
                    quantity: qty,
                    // If fully filled, we store rank = 1 (or 100) to distinguish it in pricing, or use standard rank 0/1.
                    // Let's use rank: if filled, rank is 1. If empty, rank is 0. This is very clean and standard!
                    rank: if is_filled { 1 } else { 0 },
                    max_rank: None,
                    is_mod: false,
                    is_arcane: false,
                    is_ayatan: true,
                    game_ref: game_ref.to_string(),
                });
            }
        }

        // B. Standard WFCD -> WFM lookup
        if let Some(wfcd_item) = wfcd_by_ref.get(game_ref) {
            if let Some(wfm_item) = find_wfm_match(&wfcd_item.name, &wfm_by_name) {
                let max_rank = wfcd_item.level_stats.as_ref()
                    .map(|l| (l.len() as u32).saturating_sub(1));
                    
                let is_mod = wfcd_item.category.as_deref() == Some("Mods") || game_ref.contains("/Mods/");
                let is_arcane = game_ref.contains("/CosmeticEnhancers/");

                return Some(MappedItem {
                    id: wfm_item.id.clone(),
                    slug: wfm_item.url_name.clone(),
                    name: wfm_item.item_name.clone(),
                    quantity: qty,
                    rank,
                    max_rank,
                    is_mod,
                    is_arcane,
                    is_ayatan: false,
                    game_ref: game_ref.to_string(),
                });
            }
        }

        None
    };

    // 1. Process RawUpgrades (unranked mods and arcanes)
    if let Some(ref raw_list) = inventory.raw_upgrades {
        for raw in raw_list {
            if raw.item_count > 0 {
                if let Some(mapped) = map_single(&raw.item_type, raw.item_count, 0, None) {
                    mapped_results.push(mapped);
                }
            }
        }
    }

    // 2. Process Upgrades (ranked mods and arcanes)
    if let Some(ref upg_list) = inventory.upgrades {
        for upg in upg_list {
            // Determine rank from fingerprint JSON: e.g. {"lvl": X}
            let mut rank = 0;
            if let Some(ref fp) = upg.upgrade_fingerprint {
                if let Ok(fp_val) = serde_json::from_str::<serde_json::Value>(fp) {
                    // Ignore Rivens (contain challenge or compat)
                    if fp_val.get("compat").is_none() && fp_val.get("challenge").is_none() {
                        if let Some(lvl) = fp_val.get("lvl").and_then(|v| v.as_u64()) {
                            rank = lvl as u32;
                        }
                    } else {
                        // Skip Riven mod upgrade
                        continue;
                    }
                }
            }

            if let Some(mapped) = map_single(&upg.item_type, 1, rank, None) {
                mapped_results.push(mapped);
            }
        }
    }

    // 3. Process MiscItems (e.g. stars can be found here too)
    if let Some(ref misc_list) = inventory.misc_items {
        for misc in misc_list {
            if misc.item_count > 0 {
                if let Some(mapped) = map_single(&misc.item_type, misc.item_count, 0, None) {
                    mapped_results.push(mapped);
                }
            }
        }
    }

    // 4. Process FusionTreasures (Ayatan Sculptures)
    if let Some(ref ft_list) = inventory.fusion_treasures {
        for ft in ft_list {
            if ft.item_count > 0 {
                if let Some(mapped) = map_single(&ft.item_type, ft.item_count, 0, ft.sockets) {
                    mapped_results.push(mapped);
                }
            }
        }
    }

    Ok(mapped_results)
}
