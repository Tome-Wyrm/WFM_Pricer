//! Relic uniqueName → WFM slug resolution, sourced from the cached WFCD `Relics.json`.

use std::collections::HashMap;
use std::fs;

use serde::Deserialize;

use crate::config::RELICS_CACHE_FILE;
use crate::tseprintln;

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

pub(crate) fn load_relic_map() -> HashMap<String, String> {
    let Ok(raw) = fs::read_to_string(RELICS_CACHE_FILE) else {
        tseprintln!(
            "Warning: Relics cache not found at {RELICS_CACHE_FILE}. Relics will not be mapped."
        );
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

pub(crate) fn map_relic(
    game_ref: &str,
    relic_map: &HashMap<String, String>,
) -> Option<(String, &'static str)> {
    let refinement = if game_ref.ends_with("Bronze") {
        "intact"
    } else if game_ref.ends_with("Silver") {
        "exceptional"
    } else if game_ref.ends_with("Gold") {
        "flawless"
    } else if game_ref.ends_with("Platinum") {
        "radiant"
    } else {
        return None;
    };

    let slug_base = relic_map.get(game_ref)?;
    Some((slug_base.clone(), refinement))
}
