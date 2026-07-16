use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Duration;
use tokio::time::sleep;
use toml;

use crate::config::{
    CACHE_DIR, METADATA_FILE, RELICS_CACHE_FILE, WFCD_CACHE_FILE, WFM_CACHE_FILE, FULL_ITEMS_CACHE_FILE
};
use crate::models::{MappedItem, WfcdItem, WfmItem, WfmV2Response, KeepConfig, BlacklistConfig};
use crate::vendor;
// Timestamped session logging: see src/logging.rs.
use crate::{tseprintln, tsprintln};


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

/// Mapping from a component's `uniqueName` to its parent build's `uniqueName`.
pub type BuildParentMap = std::collections::HashMap<String, String>;

/// Mapping from a build's `uniqueName` to its list of required components and quantities.
pub type BuildRequirements = std::collections::HashMap<String, Vec<(String, u32)>>;

pub const MASTERY_THRESHOLD_FRAME: u64 = 900_000;       // 1000 * 30^2
pub const MASTERY_THRESHOLD_WEAPON: u64 = 450_000;      // 500 * 30^2
pub const MASTERY_THRESHOLD_NECRAMECH: u64 = 1_600_000; // 1000 * 40^2
pub const MASTERY_THRESHOLD_OVERLEVEL_WEAPON: u64 = 800_000; // 500 * 40^2

/// True for the small, finite set of gear that ranks past 30 up to rank 40 via 5 Forma
/// (Kuva/Tenet/Coda weapons, Paracesis, and the Entrati Necramechs). Deliberately matched on
/// `display_name` rather than `unique_name` substrings — checked against real account data,
/// substring-matching the `unique_name` doesn't reliably work for this set (e.g. Paracesis has no
/// "Paracesis" anywhere in its path). The one exception is `EntratiMech`, which is a reliable
/// `unique_name` substring for both Necramechs and is kept that way to distinguish the Necramech
/// (1,600,000) threshold from the ordinary overlevel-weapon (800,000) one below.
#[must_use]
pub fn is_overlevel_gear(display_name: &str, unique_name: &str) -> bool {
    display_name.starts_with("Kuva ")
        || display_name.starts_with("Tenet ")
        || display_name.starts_with("Coda ")
        || display_name == "Paracesis"
        || unique_name.contains("EntratiMech")
}

/// Resolves the mastery XP threshold for a given item. `is_frame_tier` should come from
/// whichever equipment-array scan a caller already has on hand (see `load_mastery_and_ownership`'s
/// `frame_tier_uniques`) rather than re-deriving frame-vs-weapon a second, different way.
#[must_use]
pub fn mastery_threshold(display_name: &str, unique_name: &str, is_frame_tier: bool) -> u64 {
    if is_overlevel_gear(display_name, unique_name) {
        if unique_name.contains("EntratiMech") {
            MASTERY_THRESHOLD_NECRAMECH
        } else {
            MASTERY_THRESHOLD_OVERLEVEL_WEAPON
        }
    } else if is_frame_tier {
        MASTERY_THRESHOLD_FRAME
    } else {
        MASTERY_THRESHOLD_WEAPON
    }
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

pub const CYAN_STAR_REF: &str = "/Lotus/Types/Items/FusionTreasures/OroFusexOrnamentA";
pub const AMBER_STAR_REF: &str = "/Lotus/Types/Items/FusionTreasures/OroFusexOrnamentB";

#[derive(Debug, Clone, Deserialize)]
struct RelicMarketInfo {
    #[serde(rename = "urlName")]
    url_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RelicEntry {
    #[serde(rename = "uniqueName")]
    unique_name: String,
    #[serde(rename = "marketInfo")]
    market_info: Option<RelicMarketInfo>,
}

// ── Type aliases for complex return types ──────────────────────────────────

type LookupTables = (
    HashMap<String, WfcdItem>,
    HashMap<String, WfmItem>,
    HashMap<String, WfmItem>,
    HashMap<String, WfmItem>,
);

/// Pure helper: builds the parent map and requirements map from an already-parsed list of WFCD
/// items. Split out from `load_build_maps` so the parsing logic can be unit tested against a
/// small fixture slice without touching the filesystem.
#[must_use]
pub fn build_maps_from_items(wfcd_items: Vec<WfcdItem>) -> (BuildParentMap, BuildRequirements) {
    let mut parent_map = BuildParentMap::new();
    let mut requirements_map = BuildRequirements::new();

    for item in wfcd_items {
        if let Some(components) = item.components {
            // WFCD's `components` array lists everything needed to build the parent, not just
            // the market-sellable parts — raw crafting resources (Orokin Cell, Neurode,
            // Nanospores, Salvage, ...) are included too, and those are never tradable and
            // never appear as owned "candidate" items. Left in, they poison the recipe: a
            // build needing 10 Orokin Cell can never find that quantity in component_qty
            // during aggregation, so `possible_sets` hits 0 immediately regardless of whether
            // every real Barrel/Receiver/Stock/Blueprint is owned. Filtering to only
            // `tradable` components keeps exactly the parts that can actually be assembled
            // into (and sold as) a Set — see `WfcdComponent::tradable`.
            let tradable_components: Vec<_> = components
                .into_iter()
                .filter(|comp| comp.tradable)
                .collect();

            if tradable_components.is_empty() {
                continue;
            }

            // Store the requirements for this build
            let reqs: Vec<(String, u32)> = tradable_components
                .iter()
                .map(|comp| (comp.unique_name.clone(), comp.item_count))
                .collect();
            requirements_map.insert(item.unique_name.clone(), reqs);

            // For each component, map it back to this build
            for comp in tradable_components {
                parent_map.insert(comp.unique_name, item.unique_name.clone());
            }
        }
    }

    (parent_map, requirements_map)
}

/// Loads the build‑parent map and the component‑requirements map from the cached WFCD `All.json`.
/// Returns `(BuildParentMap, BuildRequirements)`.
///
/// # Errors
/// Returns an error if the WFCD cache file is missing, cannot be read, or cannot be parsed as
/// the expected JSON shape.
pub fn load_build_maps() -> Result<(BuildParentMap, BuildRequirements), Box<dyn Error>> {
    let cache_path = crate::config::WFCD_CACHE_FILE;
    if !std::path::Path::new(cache_path).exists() {
        return Err("WFCD cache file missing. Run update_caches first.".into());
    }
    let raw = std::fs::read_to_string(cache_path)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&raw)?;

    Ok(build_maps_from_items(wfcd_items))
}

// ── Cache management ─────────────────────────────────────────────────────────

/// Updates all local caches (WFCD All.json, WFM v2 items, Relics.json).
///
/// # Errors
/// Returns an error if:
/// - Network requests fail.
/// - GitHub commit hash cannot be fetched.
/// - File I/O operations fail.
/// - JSON parsing of cache files fails.
pub async fn update_caches() -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(CACHE_DIR)?;

    let client = reqwest::Client::new();
    tsprintln!("Checking latest WFCD commit hash...");
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

    tsprintln!("Latest WFCD Commit SHA: {latest_sha}");

    let mut cache_invalidated = true;
    if Path::new(METADATA_FILE).exists()
        && Path::new(WFCD_CACHE_FILE).exists()
        && Path::new(WFM_CACHE_FILE).exists()
        && let Ok(metadata_str) = fs::read_to_string(METADATA_FILE)
        && let Ok(metadata) = serde_json::from_str::<CacheMetadata>(&metadata_str)
        && metadata.wfcd_commit_hash == latest_sha
    {
        cache_invalidated = false;
        tsprintln!("Cache is up to date (SHA matches).");
    }

    if cache_invalidated {
        tsprintln!("Cache is missing or stale. Re-fetching data...");

        tsprintln!("Fetching WFCD All.json...");
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
        tsprintln!("WFCD All.json cached successfully.");

        tsprintln!("Fetching WFM v2 items list...");
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
        tsprintln!("WFM items list cached successfully.");

        let metadata = CacheMetadata {
            wfcd_commit_hash: latest_sha,
            last_updated: format!("{:?}", std::time::SystemTime::now()),
        };
        let metadata_str = serde_json::to_string_pretty(&metadata)?;
        fs::write(METADATA_FILE, metadata_str)?;
        tsprintln!("Cache metadata updated.");
    }

        // Vendor cache (from wiki Module:Vendors/data)
        tsprintln!("Updating vendor cache...");
        vendor::fetch_and_cache_vendors(&client).await?;
        tsprintln!("Vendor cache updated.");

    let needs_relics_refresh = cache_invalidated || !Path::new(RELICS_CACHE_FILE).exists();
    if needs_relics_refresh {
        tsprintln!("Fetching WFCD Relics.json...");
        match client
            .get("https://raw.githubusercontent.com/WFCD/warframe-items/refs/heads/master/data/json/Relics.json")
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await?;
                fs::write(RELICS_CACHE_FILE, bytes)?;
                tsprintln!("Relics.json cached successfully.");
            }
            Ok(resp) => {
                tseprintln!("Warning: Failed to fetch Relics.json ({}). Relics will not be mapped.", resp.status());
            }
            Err(e) => {
                tseprintln!("Warning: Error fetching Relics.json: {e}. Relics will not be mapped.");
            }
        }
    }

    Ok(())
}

// ── WFM item lookup helpers ───────────────────────────────────────────────────

/// Resolve the WFM item for a build's complete set (e.g. "Mag Prime" → "`mag_prime_set`").
/// Returns `None` if no such set item exists in the WFM cache.
#[must_use]
pub fn resolve_set_item(
    build_name: &str,
    wfm_by_name: &HashMap<String, WfmItem>,
) -> Option<WfmItem> {
    let set_name = format!("{build_name} Set");
    let lower = set_name.to_lowercase();
    wfm_by_name.get(&lower).cloned()
}

fn load_full_items_cache() -> Result<HashMap<String, WfmItem>, Box<dyn Error>> {
    if !Path::new(FULL_ITEMS_CACHE_FILE).exists() {
        return Ok(HashMap::new());
    }
    let content = fs::read_to_string(FULL_ITEMS_CACHE_FILE)?;
    Ok(serde_json::from_str(&content)?)
}

fn save_full_items_cache(cache: &HashMap<String, WfmItem>) -> Result<(), Box<dyn Error>> {
    let content = serde_json::to_string_pretty(cache)?;
    fs::write(FULL_ITEMS_CACHE_FILE, content)?;
    Ok(())
}

async fn fetch_full_item(
    slug: &str,
    client: &reqwest::Client,
    cache: &mut HashMap<String, WfmItem>,
) -> Result<WfmItem, Box<dyn Error>> {
    #[derive(Deserialize)]
    struct ApiResponse {
        data: WfmItem,
    }

    if let Some(item) = cache.get(slug) {
        return Ok(item.clone());
    }
    // Respect rate limit
    sleep(Duration::from_millis(400)).await;
    let url = format!("https://api.warframe.market/v2/item/{slug}");
    let resp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, "wfm-pricer-cli")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(format!("Failed to fetch full item for {}: {}", slug, resp.status()).into());
    }
    let parsed: ApiResponse = resp.json().await?;
    cache.insert(slug.to_string(), parsed.data.clone());
    Ok(parsed.data)
}

pub(crate) fn find_wfm_match<'a>(
    name: &str,
    wfm_by_name: &'a HashMap<String, WfmItem>,
) -> Option<&'a WfmItem> {
    let lower_name = name.to_lowercase();

    if let Some(item) = wfm_by_name.get(&lower_name) {
        return Some(item);
    }

    if lower_name.ends_with(" set") {
        let stripped = &lower_name[..lower_name.len() - 4];
        if let Some(item) = wfm_by_name.get(stripped) {
            return Some(item);
        }
    }

    None
}

fn is_flavour_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Items/Emotes/Syndicate/")
}

fn is_upgrade_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Upgrades/Mods/")
        || game_ref.starts_with("/Lotus/Upgrades/CosmeticEnhancers/")
}

fn is_fusion_treasure_allowed(game_ref: &str) -> bool {
    crate::mapping::AYATANS.iter().any(|a| a.game_ref == game_ref)
}

fn is_misc_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Items/Fish/")
        || game_ref.starts_with("/Lotus/Types/Items/Gems/")
        || game_ref.starts_with("/Lotus/Types/Items/PhotoBooth/")
        || game_ref.starts_with("/Lotus/Types/Items/DangerRoom/")
        || game_ref.starts_with("/Lotus/Types/Items/FusionTreasures/OroFusexOrnament")
        || game_ref.starts_with("/Lotus/Types/Items/Lenses/")
        || game_ref.starts_with("/Lotus/Types/Items/Keys/")
        || game_ref.starts_with("/Lotus/Types/Recipes/Weapons/WeaponParts/")
        || (game_ref.starts_with("/Lotus/Types/Recipes/WarframeRecipes/") && !game_ref.ends_with("Component"))
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/JuggernautPart")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/RazorbackCipherPart")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/SyringeComponent")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/GrnFlameSpearPart")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/ValenceAdapter")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/PhotoboothTile")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/DangerRoomKey")
}

fn is_relic(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Game/Projections/")
}

// ── Relic mapping ─────────────────────────────────────────────────────────────

fn load_relic_map() -> HashMap<String, String> {
    let Ok(raw) = fs::read_to_string(RELICS_CACHE_FILE) else {
        tseprintln!("Warning: Relics cache not found at {RELICS_CACHE_FILE}. Relics will not be mapped.");
        return HashMap::new();
    };

    let Ok(entries) = serde_json::from_str::<Vec<RelicEntry>>(&raw) else {
        tseprintln!("Warning: Failed to parse Relics.json cache. Relics will not be mapped.");
        return HashMap::new();
    };

    let mut map: HashMap<String, String> = HashMap::new();
    for entry in entries {
        if let Some(mi) = entry.market_info
            && let Some(url_name) = mi.url_name
            && !url_name.is_empty()
        {
            map.entry(entry.unique_name).or_insert(url_name);
        }
    }
    map
}

fn map_relic(game_ref: &str, relic_map: &HashMap<String, String>) -> Option<String> {
    let (base_unique_name, refinement) = if let Some(base) = game_ref.strip_suffix("Bronze") {
        (base, "intact")
    } else if let Some(base) = game_ref.strip_suffix("Silver") {
        (base, "exceptional")
    } else if let Some(base) = game_ref.strip_suffix("Gold") {
        (base, "flawless")
    } else if let Some(base) = game_ref.strip_suffix("Platinum") {
        (base, "radiant")
    } else {
        return None;
    };

    let slug_base = relic_map.get(base_unique_name)?;
    Some(format!("{slug_base}_{refinement}"))
}

// ── Inventory mapping helpers ──────────────────────────────────────────────────

pub(crate) fn load_lookup_tables() -> Result<LookupTables, Box<dyn Error>> {
    let wfcd_str = fs::read_to_string(WFCD_CACHE_FILE)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&wfcd_str)
        .map_err(|e| format!("Failed to parse cached WFCD All.json: {e:?}"))?;

    let wfm_str = fs::read_to_string(WFM_CACHE_FILE)?;
    let wfm_response: WfmV2Response = serde_json::from_str(&wfm_str)
        .map_err(|e| format!("Failed to parse cached WFM v2 items list: {e:?}"))?;

    let mut wfcd_by_ref = HashMap::new();
    for item in wfcd_items {
        wfcd_by_ref.insert(item.unique_name.clone(), item);
    }

    let mut wfm_by_ref = HashMap::new();
    let mut wfm_by_name = HashMap::new();
    let mut wfm_by_slug = HashMap::new();
    for item in wfm_response.data {
        if let Some(ref gr) = item.game_ref {
            wfm_by_ref.insert(gr.clone(), item.clone());
        }
        wfm_by_name.insert(item.i18n.en.name.to_lowercase(), item.clone());
        wfm_by_slug.insert(item.slug.clone(), item);
    }

    Ok((wfcd_by_ref, wfm_by_ref, wfm_by_name, wfm_by_slug))
}

fn load_keep_blacklist() -> Result<(KeepConfig, BlacklistConfig), Box<dyn Error>> {
    let keep_map = if Path::new(crate::config::KEEPLIST_FILE).exists() {
        let raw = fs::read_to_string(crate::config::KEEPLIST_FILE)?;
        toml::from_str(&raw)
            .map_err(|e| format!("Failed to parse keeplist.toml: {e:?}"))?
    } else {
        KeepConfig {
            defaults: HashMap::new(),
            items: HashMap::new(),
        }
    };

    let blacklist = if Path::new(crate::config::BLACKLIST_FILE).exists() {
        let raw = fs::read_to_string(crate::config::BLACKLIST_FILE)?;
        toml::from_str(&raw)
            .map_err(|e| format!("Failed to parse blacklist.toml: {e:?}"))?
    } else {
        BlacklistConfig::default()
    };

    Ok((keep_map, blacklist))
}

fn map_single(
    game_ref: &str,
    qty: u32,
    rank: u8,
    sockets: Option<u32>,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
) -> Option<MappedItem> {
    if (game_ref == CYAN_STAR_REF || game_ref == AMBER_STAR_REF)
        && let Some(wfm_item) = wfm_by_ref.get(game_ref)
    {
        return Some(MappedItem {
            id: wfm_item.id.clone(),
            slug: wfm_item.slug.clone(),
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: true,
            game_ref: game_ref.to_string(),
            subtypes: Vec::new(),
            bulk_tradable: wfm_item.bulk_tradable,
        });
    }

    if let Some(def) = AYATANS.iter().find(|a| a.game_ref == game_ref) {
        let _is_filled = sockets.unwrap_or(0) == def.fully_filled_mask;
        if let Some(wfm_item) = wfm_by_name.get(&def.name.to_lowercase()) {
            return Some(MappedItem {
                id: wfm_item.id.clone(),
                slug: wfm_item.slug.clone(),
                name: wfm_item.i18n.en.name.clone(),
                quantity: qty,
                rank: None,
                max_rank: None,
                rarity: String::new(),
                is_mod: false,
                is_arcane: false,
                is_ayatan: true,
                game_ref: game_ref.to_string(),
                subtypes: Vec::new(),
                bulk_tradable: wfm_item.bulk_tradable,
            });
        }
    }

    let wfm_item = wfm_by_ref.get(game_ref)
        .or_else(|| {
            wfcd_by_ref.get(game_ref).and_then(|wfcd_item| {
                find_wfm_match(&wfcd_item.name, wfm_by_name)
            })
        })?;

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
        rank: if is_mod || is_arcane { Some(rank) } else { None },
        max_rank,
        rarity: wfcd_item.and_then(|item| item.rarity.clone()).unwrap_or_default(),
        is_mod,
        is_arcane,
        is_ayatan: false,
        game_ref: game_ref.to_string(),
        subtypes: Vec::new(),
        bulk_tradable: wfm_item.bulk_tradable,
    })
}

fn process_legendary_core(item_type: &str, qty: u32) -> Option<MappedItem> {
    if item_type == "/Lotus/Upgrades/Mods/Fusers/LegendaryModFuser" {
        Some(MappedItem {
            id: "54aaf530e77989710f6b4e41".to_string(),
            slug: "legendary_fusion_core".to_string(),
            name: "Legendary Fusion Core".to_string(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: item_type.to_string(),
            subtypes: Vec::new(),
            bulk_tradable: false,
        })
    } else {
        None
    }
}

fn process_veiled_riven(
    item_type: &str,
    qty: u32,
    wfm_by_slug: &HashMap<String, WfmItem>,
) -> Option<MappedItem> {
    if !item_type.starts_with("/Lotus/Upgrades/Mods/Randomized/") {
        return None;
    }
    let riven_type = item_type.trim_start_matches("/Lotus/Upgrades/Mods/Randomized/").split('/').next().unwrap_or("");
    let slug = match riven_type {
        "Rifle"            => Some("veiled_rifle_riven_mod"),
        "Pistol"           => Some("veiled_pistol_riven_mod"),
        "Shotgun"          => Some("veiled_shotgun_riven_mod"),
        "Melee"            => Some("veiled_melee_riven_mod"),
        "Kitgun"           => Some("veiled_kitgun_riven_mod"),
        "Zaw"              => Some("veiled_zaw_riven_mod"),
        "CompanionWeapon"  => Some("veiled_companion_weapon_riven_mod"),
        _ => None,
    };
    if let Some(s) = slug && let Some(wfm_item) = wfm_by_slug.get(s) {
        return Some(MappedItem {
            id: wfm_item.id.clone(),
            slug: s.to_string(),
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: Some(0),
            max_rank: None,
            // Intentional: unveiled rivens always trade at Rare tier on WFM regardless of weapon type.
            // Do not "fix" this to read from WFCD.
            rarity: "Rare".to_string(),
            is_mod: true,
            is_arcane: false,
            is_ayatan: false,
            game_ref: item_type.to_string(),
            subtypes: Vec::new(),
            bulk_tradable: wfm_item.bulk_tradable,
        });
    }
    None
}

fn process_relic(
    item_type: &str,
    qty: u32,
    relic_map: &HashMap<String, String>,
    wfm_by_slug: &HashMap<String, WfmItem>,
) -> Option<MappedItem> {
    if !is_relic(item_type) {
        return None;
    }
    if let Some(slug) = map_relic(item_type, relic_map) && let Some(wfm_item) = wfm_by_slug.get(&slug) {
        return Some(MappedItem {
            id: wfm_item.id.clone(),
            slug,
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: item_type.to_string(),
            subtypes: Vec::new(),
            bulk_tradable: wfm_item.bulk_tradable,
        });
    }
    None
}

fn check_allowlist(
    item_type: &str,
    category_key: &str,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
) -> bool {
    match category_key {
        "FlavourItems"              => is_flavour_item_allowed(item_type),
        "RawUpgrades" | "Upgrades"  => is_upgrade_item_allowed(item_type),
        "FusionTreasures"           => is_fusion_treasure_allowed(item_type),
        "Recipes" => {
            if is_misc_item_allowed(item_type) {
                true
            } else {
                wfm_by_ref.contains_key(item_type)
                    || wfcd_by_ref.get(item_type).and_then(|wfcd_item| {
                        find_wfm_match(&wfcd_item.name, wfm_by_name)
                    }).is_some()
            }
        },
        "MiscItems" => is_misc_item_allowed(item_type),
        _ => false,
    }
}

fn parse_rank_and_sockets(item_obj: &serde_json::Map<String, serde_json::Value>) -> (u32, Option<u32>) {
    let mut rank = 0;
    if let Some(fp_str) = item_obj.get("UpgradeFingerprint").and_then(serde_json::Value::as_str)
        && let Ok(fp_val) = serde_json::from_str::<serde_json::Value>(fp_str)
    {
        if fp_val.get("compat").is_some() || fp_val.get("challenge").is_some() {
            return (rank, None);
        }
        if let Some(lvl) = fp_val.get("lvl").and_then(serde_json::Value::as_u64) {
            rank = u32::try_from(lvl).unwrap_or(0);
        }
    }
    let sockets = item_obj.get("Sockets").and_then(serde_json::Value::as_u64).map(|s| u32::try_from(s).unwrap_or(0));
    (rank, sockets)
}

fn process_item(
    element: &serde_json::Value,
    category_key: &str,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_slug: &HashMap<String, WfmItem>,
    relic_map: &HashMap<String, String>,
) -> Option<MappedItem> {
    let item_obj = element.as_object()?;
    let item_type = item_obj.get("ItemType")?.as_str()?;
    let qty = item_obj.get("ItemCount")
        .and_then(serde_json::Value::as_u64)
        .map_or(1, |q| u32::try_from(q).unwrap_or(1));
    if qty == 0 { return None; }

    let (rank, sockets) = parse_rank_and_sockets(item_obj);

    // Special cases
    if let Some(mapped) = process_legendary_core(item_type, qty) {
        return Some(mapped);
    }
    if let Some(mapped) = process_veiled_riven(item_type, qty, wfm_by_slug) {
        return Some(mapped);
    }
    if let Some(mapped) = process_relic(item_type, qty, relic_map, wfm_by_slug) {
        return Some(mapped);
    }

    // General allowlist
    if !check_allowlist(item_type, category_key, wfm_by_ref, wfcd_by_ref, wfm_by_name) {
        return None;
    }

    map_single(
        item_type,
        qty,
        u8::try_from(rank).unwrap_or(0),
        sockets,
        wfm_by_ref,
        wfm_by_name,
        wfcd_by_ref,
    )
}

/// Total raw-copy cost (including the base copy) to fuse an arcane to `rank` via
/// duplicate-consumption — triangular numbers: rank 5 needs 1+2+3+4+5+6 = 21 total copies.
pub(crate) fn arcane_rank_cost(rank: u8) -> u32 {
    match rank {
        1 => 3,
        2 => 6,
        3 => 10,
        4 => 15,
        5 => 21,
        _ => 1,
    }
}

 fn apply_keep_blacklist(
    mut item: MappedItem,
    keep_map: &KeepConfig,
    blacklist: &BlacklistConfig,
) -> Option<MappedItem> {
    if blacklist.slugs.contains(&item.slug) {
        return None;
    }
    // Mods/arcanes are no longer reserved here: a raw inventory entry is a single duplicate
    // (quantity 1 in the common case), so comparing it against `keep` per-entry silently drops
    // every duplicate independently instead of reserving one copy across the total. Keep
    // resolution for these categories now happens once, after `merge_duplicate_ranked_items`
    // pools same-slug-same-rank entries — see `apply_cross_rank_keep` below.
    if item.is_mod || item.is_arcane {
        return Some(item);
    }
    let keep_reserved = {
        let rules = keep_map.items.get(&item.slug);
        if let Some(rules) = rules {
            let rank_val = item.rank;
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
        if item.quantity <= keep_reserved {
            return None;
        }
        item.quantity -= keep_reserved;
    }
    Some(item)
     }

/// Merges `MappedItem` entries that are the same underlying item at the same rank (mods and
/// arcanes only) into one entry with a summed quantity. Without this, every leveled duplicate
/// arrives as its own `quantity: 1` entry (see `map_single`/`process_item` — the `Upgrades`
/// inventory category has no `ItemCount`, one entry per copy), and keep-reservation compared
/// against each independently instead of the true total.
fn merge_duplicate_ranked_items(items: Vec<MappedItem>) -> Vec<MappedItem> {
    let mut merged: Vec<MappedItem> = Vec::new();
    let mut index: std::collections::HashMap<(String, Option<u8>), usize> = std::collections::HashMap::new();

    for item in items {
        if item.is_mod || item.is_arcane {
            let key = (item.id.clone(), item.rank);
            if let Some(&i) = index.get(&key) {
                merged[i].quantity += item.quantity;
                continue;
            }
            index.insert(key, merged.len());
        }
        merged.push(item);
    }
    merged
}

/// Reserve `keep_total` units, drawn from the highest-rank bucket first (`variants` must
/// already be sorted rank-descending), spilling into lower ranks only if the top bucket alone
/// doesn't have enough. With `keep_total == 1` this is exactly "keep whichever copy is
/// furthest along" — a maxed copy protects itself with nothing left over; with no maxed copy,
/// the best rank-0 duplicate is held back instead.
fn apply_simple_total_reserve(variants: &mut [MappedItem], keep_total: u32) {
    let mut remaining = keep_total;
    for item in variants.iter_mut() {
        let reserve = remaining.min(item.quantity);
        item.quantity -= reserve;
        remaining -= reserve;
    }
}

/// Mods only. `variants` must already be sorted rank-descending.
///
/// If `keeplist.toml` has any rank-specific rules for this slug, each reserves exactly that
/// many units at exactly that rank, and the pooled default is skipped entirely — explicit
/// per-rank rules mean "these specific ranks are spoken for," not "add to the pool." Otherwise
/// falls back to a rank-less item override, or the `defaults.mod` category default, pooled
/// across all ranks.
fn apply_mod_keep(
    variants: &mut [MappedItem],
    item_rules: Option<&Vec<crate::models::KeepRule>>,
    default_keep: u32,
) {
    if let Some(rules) = item_rules {
        let rank_specific: Vec<&crate::models::KeepRule> =
            rules.iter().filter(|r| r.rank.is_some()).collect();
        if !rank_specific.is_empty() {
            for rule in rank_specific {
                if let Some(item) = variants.iter_mut().find(|v| v.rank == rule.rank) {
                    let reserve = rule.keep.min(item.quantity);
                    item.quantity -= reserve;
                }
            }
            return;
        }
        if let Some(rankless) = rules.iter().find(|r| r.rank.is_none()) {
            apply_simple_total_reserve(variants, rankless.keep);
            return;
        }
    }
    apply_simple_total_reserve(variants, default_keep);
}

/// Arcanes only. `variants` must already be sorted rank-descending. Reserves 1 unit of the
/// highest-ranked copy owned (the one being kept/completed), plus however many rank-0 dupes
/// are still needed to fuse it up to `max_rank`. Everything beyond that — extra maxed copies,
/// extra raw dupes — is left sellable.
fn apply_arcane_fusion_reserve(variants: &mut [MappedItem], max_rank: Option<u8>) {
    let Some(max_rank) = max_rank else { return };
    let Some(base_idx) = variants.iter().position(|v| v.quantity > 0) else { return };
    let base_rank = variants[base_idx].rank.unwrap_or(0);

    variants[base_idx].quantity -= 1;

    if base_rank >= max_rank {
        return;
    }
    let needed_raw = arcane_rank_cost(max_rank).saturating_sub(arcane_rank_cost(base_rank));
    if let Some(raw) = variants.iter_mut().find(|v| v.rank == Some(0)) {
        let reserve = needed_raw.min(raw.quantity);
        raw.quantity -= reserve;
    }
}

/// Applies keep-reservation once per underlying item, across all its rank buckets together,
/// for mods and arcanes. Must run after `merge_duplicate_ranked_items`. Non-mod/arcane items
/// pass through untouched — their keep-reservation already happened in `apply_keep_blacklist`.
fn apply_cross_rank_keep(items: Vec<MappedItem>, keep_map: &KeepConfig) -> Vec<MappedItem> {
    let mut by_item: std::collections::HashMap<String, Vec<MappedItem>> = std::collections::HashMap::new();
    let mut other = Vec::new();

    for item in items {
        if item.is_mod || item.is_arcane {
            by_item.entry(item.id.clone()).or_default().push(item);
        } else {
            other.push(item);
        }
    }

    let mut result = other;
    for (_, mut variants) in by_item {
        variants.sort_by(|a, b| b.rank.cmp(&a.rank));
        let sample_slug = variants[0].slug.clone();
        let sample_max_rank = variants[0].max_rank;
        let sample_is_arcane = variants[0].is_arcane;
        let item_rules = keep_map.items.get(&sample_slug);

        if sample_is_arcane {
            let keep_total = item_rules
                .and_then(|rules| rules.iter().find(|r| r.rank.is_none()))
                .map_or_else(
                    || keep_map.defaults.get("arcane").map_or(0, |r| r.keep),
                    |r| r.keep,
                );
            if keep_total > 0 {
              apply_arcane_fusion_reserve(&mut variants, sample_max_rank);
            }
        } else {
            let default_keep = keep_map.defaults.get("mod").map_or(0, |r| r.keep);
            apply_mod_keep(&mut variants, item_rules, default_keep);
        }

        result.extend(variants.into_iter().filter(|v| v.quantity > 0));
    }
    result
}

/// Given the inventory JSON and the WFCD lookup table, returns a set of mastered uniqueNames,
/// a set of owned‑built uniqueNames, and the set of uniqueNames classified as frame-tier (so
/// callers like the `--debug-mastery` tool can classify items the same way this function does,
/// instead of re-deriving frame-vs-weapon a second, different way).
#[must_use]
pub fn load_mastery_and_ownership(
    inventory: &serde_json::Value,
    wfcd_by_ref: &std::collections::HashMap<String, WfcdItem>,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let mut mastered_set = HashSet::new();
    let mut owned_built_set = HashSet::new();

    // ---- Build frame-tier set ----
    // NOTE: "Hoverboard" is a best-effort guess at the real inventory.json key for K-Drives,
    // based on the unique_name path prefix ("/Lotus/Types/Vehicles/Hoverboard/...") — please
    // verify against a real inventory.json export (the same way MechSuits/SpaceGuns were
    // confirmed) and correct the key name here if it's wrong. K-Drives use the frame-tier
    // 1000*R^2 formula per the Warframe Wiki, confirmed in-game with Needlenose at rank 21/30
    // reading 456,993 XP — well under 900,000, so without this key K-Drives fall through to the
    // weapon-tier default and can read as falsely "Mastered."
    let mut frame_tier_uniques = HashSet::new();
    let equipment_keys = [
        "Suits", "LongGuns", "Pistols", "Melee",
        "Archwing", "Necramech", "Sentinels", "KubrowPets",
        "MoaPets", "Hounds", "Hoverboard", "CrewShips", "SpaceSuits", "SpaceGuns", "SpaceMelee",
    ];
    if let Some(obj) = inventory.as_object() {
        for &key in &equipment_keys {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                for entry in arr {
                    if let Some(item_type) = entry.get("ItemType").and_then(|v| v.as_str()) {
                        owned_built_set.insert(item_type.to_string());
                        match key {
                            "Suits" | "Archwing" | "Necramech" | "Sentinels" | "KubrowPets" | "MoaPets" | "Hounds" | "Hoverboard" => {
                                frame_tier_uniques.insert(item_type.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // ---- Collect XP ----
    // XPInfo is the only reliable source: confirmed against real account data, it records the
    // XP value at the moment of each rank-up event and persists that value across later Forma
    // resets. The equipped item's own live "XP" field (on MechSuits, SpaceGuns, and every other
    // equipment-array entry) is the *current*, Forma-resettable affinity and is not a safe signal
    // of total achievement — do not merge it in here, even via a max(), since XPInfo already
    // captures every rank-up the item has ever actually crossed.
    let mut xp_map = std::collections::HashMap::new();
    if let Some(xp_info) = inventory.get("XPInfo").and_then(|v| v.as_array()) {
        for entry in xp_info {
            if let (Some(unique), Some(xp)) = (
                entry.get("ItemType").and_then(|v| v.as_str()),
                entry.get("XP").and_then(serde_json::Value::as_u64),
            ) {
                xp_map.insert(unique.to_string(), xp);
            }
        }
    }

    // ---- Process ----
    for (unique_name, xp_value) in xp_map {
        let display_name = wfcd_by_ref.get(&unique_name)
            .map_or("", |w| w.name.as_str());

        let threshold = mastery_threshold(display_name, &unique_name, frame_tier_uniques.contains(&unique_name));

        if xp_value >= threshold {
            mastered_set.insert(unique_name);
        }
    }

    (mastered_set, owned_built_set, frame_tier_uniques)
}

/// Build status of a parent item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    Mastered,
    BuiltUnmastered,
    NotBuilt,
    Unknown, // component not in any build map
}

/// Determine the build status of a component's parent build.
#[must_use]
pub fn get_build_status(
    component_unique_name: &str,
    parent_map: &BuildParentMap,
    mastered_set: &HashSet<String>,
    owned_built_set: &HashSet<String>,
) -> BuildStatus {
    if let Some(parent) = parent_map.get(component_unique_name) {
        if mastered_set.contains(parent) {
            BuildStatus::Mastered
        } else if owned_built_set.contains(parent) {
            BuildStatus::BuiltUnmastered
        } else {
            BuildStatus::NotBuilt
        }
    } else {
        BuildStatus::Unknown
    }
}

// ── Main mapping function ─────────────────────────────────────────────────────

/// Maps the `AlecaFrame` inventory JSON to a list of tradeable WFM items.
///
/// # Errors
/// Returns an error if:
/// - Cache files are missing.
/// - File I/O or JSON parsing fails.
/// - TOML parsing of keeplist or blacklist fails.
pub async fn map_inventory(
    inventory: &serde_json::Value,
    client: &reqwest::Client,
) -> Result<Vec<MappedItem>, Box<dyn Error>> {
    if !Path::new(WFCD_CACHE_FILE).exists() || !Path::new(WFM_CACHE_FILE).exists() {
        return Err("Cache files missing. Please run update_caches first.".into());
    }

    tsprintln!("Loading caches from disk for mapping...");
    let mut full_cache = load_full_items_cache()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, wfm_by_slug) = load_lookup_tables()?;
    let (keep_map, blacklist) = load_keep_blacklist()?;
    let relic_map = load_relic_map();

    let mut results = Vec::new();
    let allowed_keys = [
        "FlavourItems", "RawUpgrades", "Upgrades",
        "FusionTreasures", "Recipes", "MiscItems",
    ];

    // Total inventory entries across the allowed categories, used purely to render
    // "(N/total)" progress below — this does not change what gets fetched or how.
    let total_items: usize = inventory
        .as_object()
        .map(|obj| {
            allowed_keys
                .iter()
                .filter_map(|&key| obj.get(key).and_then(serde_json::Value::as_array))
                .map(std::vec::Vec::len)
                .sum()
        })
        .unwrap_or(0);
    let mut processed = 0usize;

    if let Some(obj) = inventory.as_object() {
        for &category_key in &allowed_keys {
            if let Some(arr) = obj.get(category_key).and_then(serde_json::Value::as_array) {
                for element in arr {
                    processed += 1;
                    if processed % 50 == 0 {
                        tsprintln!("Fetching item details... ({processed}/{total_items})");
                    }
                    if let Some(mut mapped) = process_item(
                        element,
                        category_key,
                        &wfm_by_ref,
                        &wfm_by_name,
                        &wfcd_by_ref,
                        &wfm_by_slug,
                        &relic_map,
                    ) {
                        // Fetch full item for this slug to get subtypes
                        match fetch_full_item(&mapped.slug, client, &mut full_cache).await {
                            Ok(full) => {
                                mapped.subtypes = full.subtypes;
                                // The lookup tables built at startup can be stale relative to
                                // the live per-item endpoint, and `bulkTradable` determines
                                // whether WFM requires `perTrade` on order creation — trust the
                                // freshly-fetched value over whatever map_single/process_item
                                // guessed from the cached tables.
                                mapped.bulk_tradable = full.bulk_tradable;
                            }
                            Err(e) => {
                                tseprintln!(
                                    "Warning: Could not fetch full item for {}: {}",
                                    mapped.slug, e
                                );
                                mapped.subtypes = Vec::new();
                            }
                        }

                        // Apply keeplist / blacklist
                        if let Some(final_item) = apply_keep_blacklist(mapped, &keep_map, &blacklist) {
                            results.push(final_item);
                        }
                    }
                }
            }
        }
    }

    // Save full items cache
    save_full_items_cache(&full_cache)?;

    let results = merge_duplicate_ranked_items(results);
    let results = apply_cross_rank_keep(results, &keep_map);
    Ok(results)
}
#[cfg(test)]
mod mapping_tests {
    use super::*;
    use crate::models::{WfmI18n, WfmEn};

    #[test]
    fn rarity_populated_from_wfcd() {
        let wfm_item = WfmItem {
            id: "test_id".into(),
            slug: "test_slug".into(),
            game_ref: Some("/Lotus/Test".into()),
            tags: vec!["mod".into()],
            max_rank: Some(10),
            i18n: WfmI18n { en: WfmEn { name: "Test Mod".into() } },
            subtypes: vec![],
            set_root: false,
            bulk_tradable: false,
            max_amber_stars: None,
            max_cyan_stars: None,
        };
        let wfcd_item = WfcdItem {
            unique_name: "/Lotus/Test".into(),
            name: "Test Mod".into(),
            level_stats: None,
            category: Some("Mods".into()),
            rarity: Some("Common".into()),
            fusion_limit: Some(10),
            components: None,
        };
        let mut wfm_by_ref = HashMap::new();
        wfm_by_ref.insert("/Lotus/Test".to_string(), wfm_item.clone());
        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("test mod".to_string(), wfm_item);
        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert("/Lotus/Test".to_string(), wfcd_item);

        let mapped = map_single(
            "/Lotus/Test",
            1,
            0,
            None,
            &wfm_by_ref,
            &wfm_by_name,
            &wfcd_by_ref,
        ).expect("mapping should succeed");

        assert_eq!(mapped.rarity, "Common");
    }
}

#[cfg(test)]
mod resolve_and_recipe_tests {
    use super::*;
    use crate::models::{WfmEn, WfmI18n};
    use std::collections::HashMap;

    #[test]
    fn resolve_set_item_finds_set_not_component() {
        let mut map = HashMap::new();
        // Simulate WFM cache entries
        let set_item = WfmItem {
            id: "set_id".into(),
            slug: "mag_prime_set".into(),
            game_ref: None,
            tags: vec![],
            max_rank: None,
            i18n: WfmI18n { en: WfmEn { name: "Mag Prime Set".into() } },
            subtypes: vec![],
            set_root: true,
            bulk_tradable: false,
            max_amber_stars: None,
            max_cyan_stars: None,
        };
        let part_item = WfmItem {
            id: "part_id".into(),
            slug: "mag_prime_chassis".into(),
            game_ref: None,
            tags: vec![],
            max_rank: None,
            i18n: WfmI18n { en: WfmEn { name: "Mag Prime Chassis".into() } },
            subtypes: vec![],
            set_root: false,
            bulk_tradable: false,
            max_amber_stars: None,
            max_cyan_stars: None,
        };
        map.insert("mag prime set".to_string(), set_item.clone());
        map.insert("mag prime chassis".to_string(), part_item);

        let resolved = resolve_set_item("Mag Prime", &map).expect("should resolve set");
        assert_eq!(resolved.slug, "mag_prime_set");
        assert_eq!(resolved.id, "set_id");
    }

    #[test]
    fn parses_known_recipe_with_real_quantities() {
        // Small fixture slice — just the Mag Prime entry and its 4 components.
        let fixture = r#"[
            {
                "uniqueName": "/Lotus/Powersuits/Mag/MagPrime",
                "name": "Mag Prime",
                "components": [
                    {"uniqueName": "/Lotus/Weapons/Tenno/Blueprints/MagPrimeBlueprint", "itemCount": 1, "tradable": true},
                    {"uniqueName": "/Lotus/Powersuits/Mag/MagPrimeChassis", "itemCount": 1, "tradable": true},
                    {"uniqueName": "/Lotus/Powersuits/Mag/MagPrimeNeuroptics", "itemCount": 1, "tradable": true},
                    {"uniqueName": "/Lotus/Powersuits/Mag/MagPrimeSystems", "itemCount": 1, "tradable": true},
                    {"uniqueName": "/Lotus/Types/Items/MiscItems/OrokinCell", "itemCount": 10, "tradable": false}
                ]
            }
        ]"#;
        let wfcd_items: Vec<WfcdItem> = serde_json::from_str(fixture).expect("fixture should parse");
        let (parent_map, requirements_map) = build_maps_from_items(wfcd_items);

        let recipe = requirements_map
            .get("/Lotus/Powersuits/Mag/MagPrime")
            .expect("Mag Prime should have a recorded recipe");
        assert_eq!(recipe.len(), 4);
        assert!(recipe.contains(&("/Lotus/Weapons/Tenno/Blueprints/MagPrimeBlueprint".to_string(), 1)));
        assert!(recipe.contains(&("/Lotus/Powersuits/Mag/MagPrimeChassis".to_string(), 1)));
        assert!(recipe.contains(&("/Lotus/Powersuits/Mag/MagPrimeNeuroptics".to_string(), 1)));
        assert!(recipe.contains(&("/Lotus/Powersuits/Mag/MagPrimeSystems".to_string(), 1)));

        // Each component should map back to the parent build.
        assert_eq!(
            parent_map.get("/Lotus/Powersuits/Mag/MagPrimeChassis"),
            Some(&"/Lotus/Powersuits/Mag/MagPrime".to_string())
        );
    }
}

#[cfg(test)]
mod build_status_tests {
    use super::*;

    fn sample_parent_map() -> BuildParentMap {
        let mut m = BuildParentMap::new();
        m.insert("part_a".to_string(), "build_x".to_string());
        m
    }

    #[test]
    fn mastered_build_no_longer_owned_is_still_mastered() {
        let parent_map = sample_parent_map();
        let mastered: HashSet<String> = ["build_x".to_string()].into_iter().collect();
        let owned: HashSet<String> = HashSet::new(); // sold the built copy
        assert_eq!(
            get_build_status("part_a", &parent_map, &mastered, &owned),
            BuildStatus::Mastered
        );
    }

    #[test]
    fn built_but_unmastered_is_built_unmastered() {
        let parent_map = sample_parent_map();
        let mastered = HashSet::new();
        let owned: HashSet<String> = ["build_x".to_string()].into_iter().collect();
        assert_eq!(
            get_build_status("part_a", &parent_map, &mastered, &owned),
            BuildStatus::BuiltUnmastered
        );
    }

    #[test]
    fn never_built_is_not_built() {
        let parent_map = sample_parent_map();
        let mastered = HashSet::new();
        let owned = HashSet::new();
        assert_eq!(
            get_build_status("part_a", &parent_map, &mastered, &owned),
            BuildStatus::NotBuilt
        );
    }

    #[test]
    fn component_with_no_known_parent_is_unknown() {
        let parent_map = sample_parent_map();
        assert_eq!(
            get_build_status("untracked_part", &parent_map, &HashSet::new(), &HashSet::new()),
            BuildStatus::Unknown
        );
    }
}

#[cfg(test)]
mod mastery_calibration_tests {
    use super::*;

    #[test]
    fn mastery_calibration_against_real_account_data() {
    // (display_name, unique_name, is_frame_tier, xp, should_be_mastered)
    //
    // is_frame_tier here reflects each item's real equipment category directly (Warframe Wiki:
    // Warframes/Archwings/Companions/Sentinels/K-Drives/Necramechs use 1000*R^2; ordinary weapons
    // use 500*R^2) rather than going through load_mastery_and_ownership's equipment-array scan —
    // that scan has its own coverage gaps (see the Hoverboard/K-Drive note above) which are a
    // separate concern from whether this threshold math itself is correct.
    let cases = [
        ("Ash", "/Lotus/Powersuits/Ninja/Ninja", true, 901_045u64, true),
        ("Acceltra", "/Lotus/Weapons/Tenno/LongGuns/SapientPrimary/SapientPrimaryWeapon", false, 450_743, true),
        // Needlenose: K-Drive deck, confirmed in-game at rank 21/30. K-Drives are frame-tier
        // (1000*R^2), not weapon-tier — at 456,993 XP this is comfortably below the frame-tier
        // rank-30 threshold of 900,000, matching the real "Not Mastered" status.
        ("Needlenose", "/Lotus/Types/Vehicles/Hoverboard/HoverboardParts/PartComponents/HoverboardCorpusB/HoverboardCorpusBDeck", true, 456_993, false),
        ("Tenet Ferrox", "/Lotus/Weapons/Corpus/BoardExec/Primary/CrpBEFerrox/CrpBEFerrox", false, 578_000, false),
        ("Coda Mire", "/Lotus/Weapons/Infested/InfestedLich/Melee/CodaMire", false, 648_000, false),
        ("Coda Motovore", "/Lotus/Weapons/Infested/InfestedLich/Melee/InfestedHammer/InfLichHammerWeapon", false, 648_000, false),
        ("Coda Pathocyst", "/Lotus/Weapons/Infested/InfestedLich/Melee/CodaPathocyst/CodaPathocyst", false, 648_000, false),
        ("Kuva Shildeg", "/Lotus/Weapons/Grineer/Melee/GrnKuvaLichScythe/GrnKuvaLichScytheWeapon", false, 648_000, false),
        ("Paracesis", "/Lotus/Weapons/Orokin/BallasSword/BallasSwordWeapon", false, 648_000, false),
        ("Tenet Grigori", "/Lotus/Weapons/Corpus/Melee/CrpBriefcaseScythe/CrpBriefcaseScythe", false, 648_000, false),
        ("Tenet Livia", "/Lotus/Weapons/Corpus/Melee/CrpBriefcase2HKatana/CrpBriefcase2HKatana", false, 648_000, false),
        // Exactly at the weapon-tier threshold (450,000) but well under the overlevel-weapon
        // threshold (800,000) it should actually be held to — a naive weapon-tier check would
        // wrongly call this mastered.
        ("Kuva Ayanga", "/Lotus/Weapons/Grineer/HeavyWeapons/GrnHeavyGrenadeLauncher", false, 450_000, false),
        ("Kuva Grattler", "/Lotus/Weapons/Grineer/KuvaLich/HeavyWeapons/Grattler/KuvaGrattler", false, 512_000, false),
        ("Bonewidow", "/Lotus/Powersuits/EntratiMech/ThanoTech", true, 900_000, false),
        // Real XPInfo value (not the live MechSuits value, which is unreliable — see the
        // load_mastery_and_ownership doc comment on why MechSuits is never read here).
        ("Voidrig", "/Lotus/Powersuits/EntratiMech/NechroTech", true, 1_024_000, false),
    ];

    for (display_name, unique_name, is_frame_tier, xp, should_be_mastered) in cases {
        let threshold = mastery_threshold(display_name, unique_name, is_frame_tier);
        assert_eq!(
            xp >= threshold,
            should_be_mastered,
            "{display_name} ({unique_name}): xp={xp}, threshold={threshold}"
        );
    }
    }
}
