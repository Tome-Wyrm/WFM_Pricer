use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use reqwest::header::USER_AGENT;

use crate::models::{
    KeepEntry, MappedItem, WfcdItem, WfmItem, WfmV2Response
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

        // Fetch WFM v2 items list
        println!("Fetching WFM v2 items list...");
        let wfm_resp_result = client
            .get("https://api.warframe.market/v2/items")
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await;
            
        let wfm_bytes = match wfm_resp_result {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await?;
                bytes.to_vec()
            }
            _ => {
                println!("WFM v2 API request failed. Attempting to use local v2_items.json fallback...");
                if Path::new("v2_items.json").exists() {
                    fs::read("v2_items.json")?
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
    wfm_by_name: &'a HashMap<String, &'a WfmItem>
) -> Option<&'a WfmItem> {
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

fn is_flavour_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Items/Emotes/Syndicate/")
}

fn is_upgrade_item_allowed(game_ref: &str) -> bool {
    // Covers both RawUpgrades (unranked mods, legendary core) and Upgrades (ranked mods)
    game_ref.starts_with("/Lotus/Upgrades/Mods/")
}

fn is_fusion_treasure_allowed(game_ref: &str) -> bool {
    crate::mapping::AYATANS.iter().any(|a| a.game_ref == game_ref)
}

fn is_misc_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Items/Fish/") ||
    game_ref.starts_with("/Lotus/Types/Items/Gems/") ||
    game_ref.starts_with("/Lotus/Types/Items/PhotoBooth/") ||
    game_ref.starts_with("/Lotus/Types/Items/DangerRoom/") ||
    game_ref.starts_with("/Lotus/Types/Items/FusionTreasures/OroFusexOrnament") || // Ayatan Stars
    game_ref.starts_with("/Lotus/Types/Items/Lenses/") ||
    game_ref.starts_with("/Lotus/Types/Items/Keys/") ||
    game_ref.starts_with("/Lotus/Types/Recipes/Weapons/WeaponParts/") ||
    (game_ref.starts_with("/Lotus/Types/Recipes/WarframeRecipes/") && !game_ref.ends_with("Component")) || // Warframe parts, but not the final "Component" for built frames
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/JuggernautPart") ||
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/RazorbackCipherPart") ||
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/SyringeComponent") || // Nav Coordinates
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/GrnFlameSpearPart") || // Vay Hek beacons
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/ValenceAdapter") ||
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/PhotoboothTile") || // Older scene items sometimes appear here
    game_ref.starts_with("/Lotus/Types/Items/MiscItems/DangerRoomKey")
}

fn is_relic(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Game/Projections/")
}

/// Performs item intersection and maps raw inventory to WFM tradeable items.
pub fn map_inventory(inventory: &serde_json::Value) -> Result<Vec<MappedItem>, Box<dyn Error>> {
    // 1. Load caches
    if !Path::new(WFCD_CACHE_FILE).exists() || !Path::new(WFM_CACHE_FILE).exists() {
        return Err("Cache files missing. Please run update_caches first.".into());
    }

    println!("Loading caches from disk for mapping...");
    let wfcd_str = fs::read_to_string(WFCD_CACHE_FILE)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&wfcd_str)
        .map_err(|e| format!("Failed to parse cached WFCD All.json: {:?}", e))?;

    let wfm_str = fs::read_to_string(WFM_CACHE_FILE)?;
    let wfm_response: WfmV2Response = serde_json::from_str(&wfm_str)
        .map_err(|e| format!("Failed to parse cached WFM v2 items list: {:?}", e))?;

    // Create lookup tables
    let mut wfcd_by_ref = HashMap::new();
    for item in &wfcd_items {
        wfcd_by_ref.insert(item.unique_name.clone(), item);
    }

    let mut wfm_by_ref = HashMap::new();
    let mut wfm_by_name = HashMap::new();
    let mut wfm_by_slug = HashMap::new(); // New map for slug lookup
    for item in &wfm_response.data {
        if let Some(ref gr) = item.game_ref {
            wfm_by_ref.insert(gr.clone(), item);
        }
        wfm_by_name.insert(item.i18n.en.name.to_lowercase(), item);
        wfm_by_slug.insert(item.slug.clone(), item); // Insert slug for veiled rivens
    }

    // 2. Load keeplist.json (user-defined per slug+rank reserves)
    // Key: (slug, rank) -> copies to keep
    let keep_map: HashMap<(String, u32), u32> = if Path::new("keeplist.json").exists() {
        let raw = fs::read_to_string("keeplist.json")?;
        let entries: Vec<KeepEntry> = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse keeplist.json: {:?}", e))?;
        let mut map = HashMap::new();
        for entry in entries {
            *map.entry((entry.slug, entry.rank)).or_insert(0) += entry.keep;
        }
        map
    } else {
        HashMap::new()
    };

    // 3. Load blacklist.json
    let blacklist: std::collections::HashSet<String> = if Path::new("blacklist.json").exists() {
        let raw = fs::read_to_string("blacklist.json")?;
        let entries: Vec<crate::models::BlacklistEntry> = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse blacklist.json: {:?}", e))?;
        entries.into_iter().map(|e| e.slug).collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut mapped_results = Vec::new();

    // Helper closure to map a single game_ref and quantity/rank combination
    // `category` and `excluded_categories` parameters removed, as filtering happens upstream.
    let map_single = |
        game_ref: &str, 
        qty: u32, 
        rank: u32, 
        sockets: Option<u32>,
        wfm_by_ref: &HashMap<String, &WfmItem>,
        wfm_by_name: &HashMap<String, &WfmItem>,
        wfcd_by_ref: &HashMap<String, &WfcdItem>
    | -> Option<MappedItem> {
        
        // A. Check static Ayatans and Stars first (these are primarily from MiscItems and FusionTreasures,
        //    but the `game_ref` based match here is robust).
        if game_ref == CYAN_STAR_REF {
            return Some(MappedItem {
                id: "58ca59c071d7d022b7405e32".to_string(), // WFM id for Cyan Star
                slug: "ayatan_cyan_star".to_string(),
                name: "Ayatan Cyan Star".to_string(),
                quantity: qty,
                rank: None, // Stars have no rank
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
                rank: None, // Stars have no rank
                max_rank: None,
                is_mod: false,
                is_arcane: false,
                is_ayatan: true,
                game_ref: game_ref.to_string(),
            });
        }

        if let Some(def) = AYATANS.iter().find(|a| a.game_ref == game_ref) {
            let is_filled = sockets.unwrap_or(0) == def.fully_filled_mask;
            if let Some(wfm_item) = wfm_by_name.get(&def.name.to_lowercase()) {
                return Some(MappedItem {
                    id: wfm_item.id.clone(),
                    slug: wfm_item.slug.clone(),
                    name: wfm_item.i18n.en.name.clone(),
                    quantity: qty,
                    rank: None, // Ayatan sculptures are not mods/arcanes, so rank is None per request
                    max_rank: None,
                    is_mod: false,
                    is_arcane: false,
                    is_ayatan: true,
                    game_ref: game_ref.to_string(),
                });
            }
        }

        // Perform Dual lookup: by gameRef first, then by name matching
        let wfm_item = wfm_by_ref.get(game_ref)
            .copied()
            .or_else(|| {
                wfcd_by_ref.get(game_ref).and_then(|wfcd_item| {
                    find_wfm_match(&wfcd_item.name, wfm_by_name)
                })
            })?;

        // Retrieve metadata from WFCD item if available
        let wfcd_item = wfcd_by_ref.get(game_ref);
        let max_rank = wfm_item.max_rank.or_else(|| {
            wfcd_item.and_then(|item| {
                item.level_stats.as_ref().map(|l| (l.len() as u32).saturating_sub(1))
            })
        });

        let is_mod = wfm_item.tags.contains(&"mod".to_string()) 
            || game_ref.contains("/Mods/") 
            || wfcd_item.map_or(false, |item| item.category.as_deref() == Some("Mods"));
            
        let is_arcane = wfm_item.tags.contains(&"arcane".to_string()) 
            || game_ref.contains("/CosmeticEnhancers/");

        Some(MappedItem {
            id: wfm_item.id.clone(),
            slug: wfm_item.slug.clone(),
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: if is_mod || is_arcane { Some(rank) } else { None }, // Rank only for mods/arcanes
            max_rank,
            is_mod,
            is_arcane,
            is_ayatan: false,
            game_ref: game_ref.to_string(),
        })
    };

    // Iterate through specific allowed inventory categories
    let allowed_inventory_keys = [
        "FlavourItems", "RawUpgrades", "Upgrades", 
        "FusionTreasures", "Recipes", "MiscItems"
    ];

    if let Some(obj) = inventory.as_object() {
        for &category_key in &allowed_inventory_keys {
            if let Some(val) = obj.get(category_key) {
                if let Some(arr) = val.as_array() {
                    for element in arr {
                        if let Some(item_obj) = element.as_object() {
                            if let Some(item_type) = item_obj.get("ItemType").and_then(|v| v.as_str()) {
                                
                                let qty = item_obj.get("ItemCount")
                                    .and_then(|v| v.as_u64())
                                    .map(|q| q as u32)
                                    .unwrap_or(1);
                                    
                                if qty == 0 {
                                    continue;
                                }
                                
                                let mut rank = 0;
                                if let Some(fp_str) = item_obj.get("UpgradeFingerprint").and_then(|v| v.as_str()) {
                                    if let Ok(fp_val) = serde_json::from_str::<serde_json::Value>(fp_str) {
                                        // Skip standard Rivens (non-veiled) - handled later for veiled
                                        if fp_val.get("compat").is_some() || fp_val.get("challenge").is_some() {
                                            continue;
                                        }
                                        if let Some(lvl) = fp_val.get("lvl").and_then(|v| v.as_u64()) {
                                            rank = lvl as u32;
                                        }
                                    }
                                }
                                
                                let sockets = item_obj.get("Sockets").and_then(|v| v.as_u64()).map(|s| s as u32);
                                
                                let mut mapped_item: Option<MappedItem> = None;

                                // Special case: Legendary Core
                                if item_type == "/Lotus/Upgrades/Mods/Fusers/LegendaryModFuser" {
                                    mapped_item = Some(MappedItem {
                                        id: "54aaf530e77989710f6b4e41".to_string(), // WFM id for Legendary Fusion Core
                                        slug: "legendary_fusion_core".to_string(),
                                        name: "Legendary Fusion Core".to_string(),
                                        quantity: qty,
                                        rank: None,
                                        max_rank: None,
                                        is_mod: false,
                                        is_arcane: false,
                                        is_ayatan: false,
                                        game_ref: item_type.to_string(),
                                    });
                                } 
                                // Special case: Veiled Rivens
                                else if item_type.starts_with("/Lotus/Upgrades/Mods/Randomized/") {
                                    let riven_type_segment = item_type.trim_start_matches("/Lotus/Upgrades/Mods/Randomized/").split('/').next().unwrap_or("");
                                    let slug = match riven_type_segment {
                                        "Rifle" => Some("veiled_rifle_riven_mod"),
                                        "Pistol" => Some("veiled_pistol_riven_mod"),
                                        "Shotgun" => Some("veiled_shotgun_riven_mod"),
                                        "Melee" => Some("veiled_melee_riven_mod"),
                                        "Kitgun" => Some("veiled_kitgun_riven_mod"),
                                        "Zaw" => Some("veiled_zaw_riven_mod"),
                                        "CompanionWeapon" => Some("veiled_companion_weapon_riven_mod"),
                                        _ => None, // Unknown riven type, skip
                                    };

                                    if let Some(s) = slug {
                                        if let Some(wfm_item) = wfm_by_slug.get(s) {
                                            mapped_item = Some(MappedItem {
                                                id: wfm_item.id.clone(),
                                                slug: wfm_item.slug.clone(),
                                                name: wfm_item.i18n.en.name.clone(),
                                                quantity: qty,
                                                rank: Some(0), // Veiled rivens are always rank 0
                                                max_rank: None,
                                                is_mod: true,
                                                is_arcane: false,
                                                is_ayatan: false,
                                                game_ref: item_type.to_string(),
                                            });
                                        }
                                    }
                                }
                                // General Allowlist filtering
                                else {
                                    let is_allowed = match category_key {
                                        "FlavourItems" => is_flavour_item_allowed(item_type),
                                        "RawUpgrades" | "Upgrades" => is_upgrade_item_allowed(item_type),
                                        "FusionTreasures" => is_fusion_treasure_allowed(item_type),
                                        "Recipes" => {
                                            // For "Recipes" category, apply MiscItems filters first
                                            if is_misc_item_allowed(item_type) {
                                                true
                                            } else {
                                                // Fallback for generic recipes: WFM match acts as gatekeeper
                                                wfm_by_ref.contains_key(item_type) || 
                                                wfcd_by_ref.get(item_type).and_then(|wfcd_item| {
                                                    find_wfm_match(&wfcd_item.name, &wfm_by_name)
                                                }).is_some()
                                            }
                                        },
                                        "MiscItems" => {
                                            if is_relic(item_type) {
                                                // TODO: Relic matching requires a wiki relics.json lookup table mapping
                                                // projection uniqueName to WFM urlName, with Bronze/Silver/Gold/Platinum
                                                // suffix -> intact/exceptional/flawless/radiant.
                                                false // Skip relics for now
                                            } else {
                                                is_misc_item_allowed(item_type)
                                            }
                                        },
                                        _ => false, // Should not be reached with predefined `allowed_inventory_keys`
                                    };

                                    if is_allowed {
                                        mapped_item = map_single(item_type, qty, rank, sockets, &wfm_by_ref, &wfm_by_name, &wfcd_by_ref);
                                    }
                                }

                                if let Some(mut mapped) = mapped_item {
                                    // Apply blacklist
                                    if blacklist.contains(&mapped.slug) {
                                        continue;
                                    }

                                    // Apply keeplist: subtract reserved copies for this slug+rank
                                    let keep_rank_for_key = mapped.rank.unwrap_or(0); // Convert Option<u32> to u32 for keep_map key
                                    let keep_key = (mapped.slug.clone(), keep_rank_for_key);
                                    if let Some(&reserved) = keep_map.get(&keep_key) {
                                        if mapped.quantity <= reserved {
                                            continue; // All copies reserved, skip entirely
                                        }
                                        mapped.quantity -= reserved;
                                    }
                                    mapped_results.push(mapped);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(mapped_results)
}
