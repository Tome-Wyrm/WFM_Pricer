//! Local cache management: refreshing the WFCD/WFM/Vendor/Relics caches from their upstream
//! sources, and the per-item "full item" (subtypes, live `bulkTradable`) cache used during
//! inventory mapping.

use crate::AppResult;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::config::{
    CACHE_DIR, FULL_ITEMS_CACHE_FILE, METADATA_FILE, RELICS_CACHE_FILE, WFCD_CACHE_FILE,
    WFM_CACHE_FILE,
};
use crate::models::WfmItem;
use crate::vendor;
use crate::wfm_client::wfm_error_for_status;
use crate::{tseprintln, tsprintln};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub wfcd_commit_hash: String,
    pub last_updated: String,
}

/// Updates all local caches (WFCD All.json, WFM v2 items, Relics.json).
///
/// # Errors
/// Returns an error if:
/// - Network requests fail.
/// - GitHub commit hash cannot be fetched.
/// - File I/O operations fail.
/// - JSON parsing of cache files fails.
pub async fn update_caches() -> AppResult<()> {
    fs::create_dir_all(CACHE_DIR)?;

    let client = crate::http::shared_client();
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
            Ok(resp) if resp.status().is_success() => resp.bytes().await?.to_vec(),
            _ => {
                return Err(
                    "WFM v2 items API request failed and no cache exists. Check your connection."
                        .into(),
                );
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
    vendor::fetch_and_cache_vendors(client).await?;
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

pub(crate) fn load_full_items_cache() -> AppResult<HashMap<String, WfmItem>> {
    if !Path::new(FULL_ITEMS_CACHE_FILE).exists() {
        return Ok(HashMap::new());
    }
    let content = fs::read_to_string(FULL_ITEMS_CACHE_FILE)?;
    Ok(serde_json::from_str(&content)?)
}

pub(crate) fn save_full_items_cache(cache: &HashMap<String, WfmItem>) -> AppResult<()> {
    let content = serde_json::to_string_pretty(cache)?;
    fs::write(FULL_ITEMS_CACHE_FILE, content)?;
    Ok(())
}

pub(crate) async fn fetch_full_item(
    slug: &str,
    client: &reqwest::Client,
    cache: &mut HashMap<String, WfmItem>,
) -> AppResult<WfmItem> {
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
        return Err(Box::new(
            wfm_error_for_status(resp, format!("fetching full item {slug}")).await,
        ));
    }
    let parsed: ApiResponse = resp.json().await?;
    cache.insert(slug.to_string(), parsed.data.clone());
    Ok(parsed.data)
}
