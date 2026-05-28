use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use toml;

use crate::models::{MappedItem, WfcdItem, WfmItem, WfmV2Response};
use crate::config::{CACHE_DIR, METADATA_FILE, RELICS_CACHE_FILE, WFCD_CACHE_FILE, WFM_CACHE_FILE};


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

// ── Relic cache structures ────────────────────────────────────────────────────

/// Minimal market info block from WFCD's Relics.json.
#[derive(Debug, Clone, Deserialize)]
struct RelicMarketInfo {
    /// WFM `url_name` **base** slug, e.g. `"lith_b4_relic"` (no refinement suffix).
    #[serde(rename = "urlName")]
    url_name: Option<String>,
}

/// One entry from WFCD's Relics.json.
///
/// The file may contain multiple entries sharing the same `uniqueName` (one
/// per reward slot).  We deduplicate by `uniqueName`, keeping the first
/// `marketInfo` encountered.
#[derive(Debug, Clone, Deserialize)]
struct RelicEntry {
    #[serde(rename = "uniqueName")]
    unique_name: String,
    #[serde(rename = "marketInfo")]
    market_info: Option<RelicMarketInfo>,
}

// ── Cache management ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)] // This function needs a significant refactor to reduce line count.
/// Ensures all necessary cache data is fetched, updated, and verified.
///
/// # Errors
/// Returns an error if any network request fails, file operations fail,
/// or JSON parsing of cache files fails.
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

    println!("Latest WFCD Commit SHA: {latest_sha}");

    // 2. Check if cache is still valid
    let mut cache_invalidated = true;
    if Path::new(METADATA_FILE).exists()
        && Path::new(WFCD_CACHE_FILE).exists()
        && Path::new(WFM_CACHE_FILE).exists()
        && let Ok(metadata_str) = fs::read_to_string(METADATA_FILE)
        && let Ok(metadata) = serde_json::from_str::<CacheMetadata>(&metadata_str)
        && metadata.wfcd_commit_hash == latest_sha
    {
        cache_invalidated = false;
        println!("Cache is up to date (SHA matches).");
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
                resp.bytes().await?.to_vec()
            }
            _ => {
                return Err("WFM v2 items API request failed and no cache exists. Check your connection.".into());
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

    // 4. Fetch Relics.json when the main caches were just refreshed, or
    //    if the relic cache file is missing (e.g. deleted manually).
    let needs_relics_refresh = cache_invalidated || !Path::new(RELICS_CACHE_FILE).exists();
    if needs_relics_refresh {
        println!("Fetching WFCD Relics.json...");
        match client
            .get("https://raw.githubusercontent.com/WFCD/warframe-items/refs/heads/master/data/json/Relics.json")
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await?;
                fs::write(RELICS_CACHE_FILE, bytes)?;
                println!("Relics.json cached successfully.");
            }
            Ok(resp) => {
                // Non-fatal: relics just won't be mapped this session
                eprintln!("Warning: Failed to fetch Relics.json ({}). Relics will not be mapped.", resp.status());
            }
            Err(e) => {
                eprintln!("Warning: Error fetching Relics.json: {e}. Relics will not be mapped.");
            }
        }
    }

    Ok(())
}

// ── WFM item lookup helpers ───────────────────────────────────────────────────

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

// ── Relic mapping ─────────────────────────────────────────────────────────────

/// Builds a `HashMap<uniqueName_base, wfm_slug_base>` from the cached Relics.json.
///
/// The WFCD file may contain duplicate `uniqueName` entries (one per reward slot
/// per relic); we deduplicate by keeping the first valid `marketInfo.urlName`
/// encountered for each uniqueName.
///
/// Inventory paths include the refinement suffix (`Bronze` / `Silver` / `Gold` /
/// `Platinum`); WFCD uniqueNames do **not** — so the keys in this map are the
/// paths **without** the refinement suffix.
fn load_relic_map() -> HashMap<String, String> {
    let Ok(raw) = fs::read_to_string(RELICS_CACHE_FILE) else {
        eprintln!("Warning: Relics cache not found at {RELICS_CACHE_FILE}. Relics will not be mapped.");
        return HashMap::new();
    };

    let Ok(entries) = serde_json::from_str::<Vec<RelicEntry>>(&raw) else {
        eprintln!("Warning: Failed to parse Relics.json cache. Relics will not be mapped.");
        return HashMap::new();
    };

    let mut map: HashMap<String, String> = HashMap::new();
    for entry in entries {
        if let Some(mi) = entry.market_info
            && let Some(url_name) = mi.url_name
            && !url_name.is_empty()
        {
            // Keep first encountered (deduplication)
            map.entry(entry.unique_name).or_insert(url_name);
        }
    }
    map
}

/// Maps a relic inventory path to its WFM slug, or returns `None` if unmappable.
///
/// The inventory encodes refinement as a path suffix:
/// | Suffix     | WFM refinement |
/// |------------|----------------|
/// | `Bronze`   | `intact`       |
/// | `Silver`   | `exceptional`  |
/// | `Gold`     | `flawless`     |
/// | `Platinum` | `radiant`      |
///
/// The WFCD uniqueName for a relic is the path **without** this suffix.
/// We strip the suffix, look up the slug base, then rejoin: `"{base}_{refinement}"`.
fn map_relic(game_ref: &str, relic_map: &HashMap<String, String>) -> Option<String> {
    // Detect refinement and compute the base uniqueName by stripping the suffix
    let (base_unique_name, refinement) = if let Some(base) = game_ref.strip_suffix("Bronze") {
        (base, "intact")
    } else if let Some(base) = game_ref.strip_suffix("Silver") {
        (base, "exceptional")
    } else if let Some(base) = game_ref.strip_suffix("Gold") {
        (base, "flawless")
    } else if let Some(base) = game_ref.strip_suffix("Platinum") {
        (base, "radiant")
    } else {
        return None; // Unknown / non-standard refinement suffix
    };

    let slug_base = relic_map.get(base_unique_name)?;
    Some(format!("{slug_base}_{refinement}"))
}

// ── Inventory mapping ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)] // This function needs a significant refactor to reduce line count.
/// Performs item intersection and maps raw inventory to WFM tradeable items.
///
/// # Errors
/// Returns an error if cache files are missing, cannot be read,
/// or JSON parsing of cache, keeplist, or blacklist files fails.
pub fn map_inventory(inventory: &serde_json::Value) -> Result<Vec<MappedItem>, Box<dyn Error>> {
    // 1. Load caches
    if !Path::new(WFCD_CACHE_FILE).exists() || !Path::new(WFM_CACHE_FILE).exists() {
        return Err("Cache files missing. Please run update_caches first.".into());
    }

    println!("Loading caches from disk for mapping...");
    let wfcd_str = fs::read_to_string(WFCD_CACHE_FILE)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&wfcd_str)
        .map_err(|e| format!("Failed to parse cached WFCD All.json: {e:?}"))?;

    let wfm_str = fs::read_to_string(WFM_CACHE_FILE)?;
    let wfm_response: WfmV2Response = serde_json::from_str(&wfm_str)
        .map_err(|e| format!("Failed to parse cached WFM v2 items list: {e:?}"))?;

    // Create lookup tables
    let mut wfcd_by_ref = HashMap::new();
    for item in &wfcd_items {
        wfcd_by_ref.insert(item.unique_name.clone(), item);
    }

    let mut wfm_by_ref = HashMap::new();
    let mut wfm_by_name = HashMap::new();
    let mut wfm_by_slug = HashMap::new();
    for item in &wfm_response.data {
        if let Some(ref gr) = item.game_ref {
            wfm_by_ref.insert(gr.clone(), item);
        }
        wfm_by_name.insert(item.i18n.en.name.to_lowercase(), item);
        wfm_by_slug.insert(item.slug.clone(), item);
    }

    // 2. Load keeplist.toml (user-defined per slug+rank reserves)
    let keep_map: crate::models::KeepConfig = if Path::new(crate::config::KEEPLIST_FILE).exists() {
        let raw = fs::read_to_string(crate::config::KEEPLIST_FILE)?;
        toml::from_str(&raw)
            .map_err(|e| format!("Failed to parse keeplist.toml: {e:?}"))?
    } else {
        crate::models::KeepConfig { defaults: HashMap::default(), items: HashMap::default() }
    };

    // 3. Load blacklist.toml
    let blacklist: crate::models::BlacklistConfig = if Path::new(crate::config::BLACKLIST_FILE).exists() {
        let raw = fs::read_to_string(crate::config::BLACKLIST_FILE)?;
        toml::from_str(&raw)
            .map_err(|e| format!("Failed to parse blacklist.toml: {e:?}"))?
    } else {
        crate::models::BlacklistConfig::default()
    };

    // 4. Load relic map (uniqueName_base -> wfm_slug_base)
    let relic_map = load_relic_map();

    let mut mapped_results = Vec::new();

    // Helper closure to map a single game_ref and quantity/rank combination
    let map_single = |
        game_ref: &str,
        qty: u32,
        rank: u8,
        sockets: Option<u32>,
        wfm_by_ref: &HashMap<String, &WfmItem>,
        wfm_by_name: &HashMap<String, &WfmItem>,
        wfcd_by_ref: &HashMap<String, &WfcdItem>
    | -> Option<MappedItem> {

        // A. Check static Ayatans and Stars first (these are primarily from MiscItems and FusionTreasures,
        //    but the `game_ref` based match here is robust).
        if game_ref == CYAN_STAR_REF || game_ref == AMBER_STAR_REF {
                    if let Some(wfm_item) = wfm_by_ref.get(game_ref) {
                        return Some(MappedItem {
                            id: wfm_item.id.clone(),
                            slug: wfm_item.slug.clone(),
                            name: wfm_item.i18n.en.name.clone(),
                            quantity: qty,
                            rank: None,
                            max_rank: None,
                            rarity: "".to_string(),
                            is_mod: false,
                            is_arcane: false,
                            is_ayatan: true,
                            game_ref: game_ref.to_string(),
                        });
                    }
                }

        if let Some(def) = AYATANS.iter().find(|a| a.game_ref == game_ref) {
            let _is_filled = sockets.unwrap_or(0) == def.fully_filled_mask;
            if let Some(wfm_item) = wfm_by_name.get(&def.name.to_lowercase()) {
                return Some(MappedItem {
                    id: wfm_item.id.clone(),
                    slug: wfm_item.slug.clone(),
                    name: wfm_item.i18n.en.name.clone(),
                    quantity: qty,
                    rank: None, // Ayatan sculptures are not mods/arcanes, so rank is None per request
                    max_rank: None,
                    rarity: "".to_string(),
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
        let max_rank: Option<u8> = wfm_item.max_rank
            .and_then(|r| u8::try_from(r).ok())
            .or_else(|| {
                wfcd_item.and_then(|item| {
                    item.level_stats.as_ref().map(|l| {
                        u8::try_from(l.len()).unwrap_or(0).saturating_sub(1)
                    })
                })
            });

        let is_mod = wfm_item.tags.contains(&"mod".to_string())
            || game_ref.contains("/Mods/")
            || wfcd_item.is_some_and(|item| item.category.as_deref() == Some("Mods"));

        let is_arcane = wfm_item.tags.contains(&"arcane".to_string())
            || game_ref.contains("/CosmeticEnhancers/");

        Some(MappedItem {
            id: wfm_item.id.clone(),
            slug: wfm_item.slug.clone(),
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: if is_mod || is_arcane { Some(rank) } else { None }, // Rank only for mods/arcanes
            max_rank,
            rarity: "".to_string(),
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
            if let Some(val) = obj.get(category_key)
                && let Some(arr) = val.as_array()
            {
                for element in arr {
                    if let Some(item_obj) = element.as_object()
                        && let Some(item_type) = item_obj.get("ItemType").and_then(|v| v.as_str())
                    {
                        let qty = item_obj.get("ItemCount")
                            .and_then(serde_json::Value::as_u64)
                            .map_or(1, |q| u32::try_from(q).unwrap_or(1));

                        if qty == 0 {
                            continue;
                        }

                        let mut rank: u32 = 0;
                        if let Some(fp_str) = item_obj.get("UpgradeFingerprint").and_then(|v| v.as_str())
                            && let Ok(fp_val) = serde_json::from_str::<serde_json::Value>(fp_str)
                        {
                            // Skip standard Rivens (non-veiled) - handled later for veiled
                            if fp_val.get("compat").is_some() || fp_val.get("challenge").is_some() {
                                continue;
                            }
                            if let Some(lvl) = fp_val.get("lvl").and_then(serde_json::Value::as_u64) {
                                rank = u32::try_from(lvl).unwrap_or(0);
                            }
                        }

                        let sockets = item_obj.get("Sockets").and_then(serde_json::Value::as_u64).map(|s| u32::try_from(s).unwrap_or(0));

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
                                rarity: "".to_string(),
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
                                "Rifle"            => Some("veiled_rifle_riven_mod"),
                                "Pistol"           => Some("veiled_pistol_riven_mod"),
                                "Shotgun"          => Some("veiled_shotgun_riven_mod"),
                                "Melee"            => Some("veiled_melee_riven_mod"),
                                "Kitgun"           => Some("veiled_kitgun_riven_mod"),
                                "Zaw"              => Some("veiled_zaw_riven_mod"),
                                "CompanionWeapon"  => Some("veiled_companion_weapon_riven_mod"),
                                _ => None, // Unknown riven type, skip
                            };

                            if let Some(s) = slug
                                && let Some(wfm_item) = wfm_by_slug.get(s)
                            {
                                mapped_item = Some(MappedItem {
                                    id: wfm_item.id.clone(),
                                    slug: wfm_item.slug.clone(),
                                    name: wfm_item.i18n.en.name.clone(),
                                    quantity: qty,
                                    rank: Some(0), // Veiled rivens are always rank 0
                                    max_rank: None,
                                    rarity: "Rare".to_string(),
                                    is_mod: true,
                                    is_arcane: false,
                                    is_ayatan: false,
                                    game_ref: item_type.to_string(),
                                });
                            }
                        }
                        // Special case: Relics
                        // Handled here (before general allowlist) because we need the relic_map
                        // and the refinement-based slug construction that map_single() cannot do.
                        else if is_relic(item_type) {
                            if let Some(slug) = map_relic(item_type, &relic_map)
                                && let Some(wfm_item) = wfm_by_slug.get(slug.as_str())
                            {
                                mapped_item = Some(MappedItem {
                                    id: wfm_item.id.clone(),
                                    slug,
                                    name: wfm_item.i18n.en.name.clone(),
                                    quantity: qty,
                                    // Refinement is already encoded in the slug; rank is not applicable
                                    rank: None,
                                    max_rank: None,
                                    rarity: "".to_string(),
                                    is_mod: false,
                                    is_arcane: false,
                                    is_ayatan: false,
                                    game_ref: item_type.to_string(),
                                });
                            }
                        }
                        // General Allowlist filtering
                        else {
                            let is_allowed = match category_key {
                                "FlavourItems"              => is_flavour_item_allowed(item_type),
                                "RawUpgrades" | "Upgrades"  => is_upgrade_item_allowed(item_type),
                                "FusionTreasures"           => is_fusion_treasure_allowed(item_type),
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
                                "MiscItems" => is_misc_item_allowed(item_type),
                                _ => false, // Should not be reached with predefined `allowed_inventory_keys`
                            };

                            if is_allowed {
                                mapped_item = map_single(item_type, qty, u8::try_from(rank).unwrap_or(0), sockets, &wfm_by_ref, &wfm_by_name, &wfcd_by_ref);
                            }
                        }

                        if let Some(mut mapped) = mapped_item {
                            // Apply blacklist
                            if blacklist.slugs.contains(&mapped.slug) {
                                continue;
                            }

                            // Apply keeplist: subtract reserved copies for this slug+rank
                            let keep_reserved = {
                                let rules = keep_map.items.get(&mapped.slug);
                                if let Some(rules) = rules {
                                    let rank_val = mapped.rank;
                                    if let Some(rank) = rank_val {
                                        rules.iter().find(|r| r.rank == Some(rank))
                                            .or_else(|| rules.iter().find(|r| r.rank.is_none()))
                                            .map_or(0, |r| r.keep)
                                    } else {
                                        rules.iter().find(|r| r.rank.is_none())
                                            .map_or(0, |r| r.keep)
                                    }
                                } else {
                                    0
                                }
                            };
                            if keep_reserved > 0 {
                                if mapped.quantity <= keep_reserved {
                                    continue;
                                }
                                mapped.quantity -= keep_reserved;
                            }
                            mapped_results.push(mapped);
                        }
                    }
                }
            }
        }
    }

    Ok(mapped_results)
}
