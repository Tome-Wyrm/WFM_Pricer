// src/vendor/network.rs
//! Fetches `Module:Vendors/data` from the Warframe wiki (revid-cached) and parses it
//! into `RawVendor`s.
use super::lua::{LuaKey, parse, tokenize};
use super::raw::{RawVendor, parse_raw_vendor, write_vendor_cache};
use crate::AppResult;
use crate::tsprintln;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// ---- Revid caching ----

#[derive(Debug, Serialize, Deserialize)]
struct RevidCache {
    revid: u64,
}

/// Reads the cached revid from disk, if present.
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed as JSON.
pub fn read_cached_revid() -> AppResult<Option<u64>> {
    let path = Path::new(crate::config::VENDOR_REVID_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    let cache: RevidCache = serde_json::from_str(&content)?;
    Ok(Some(cache.revid))
}

/// Writes the given revid to the cache file.
/// # Errors
/// Returns an error if the cache file cannot be written or serialized to JSON.
pub fn write_cached_revid(revid: u64) -> AppResult<()> {
    let path = Path::new(crate::config::VENDOR_REVID_FILE);
    let cache = RevidCache { revid };
    let content = serde_json::to_string_pretty(&cache)?;
    fs::write(path, content)?;
    Ok(())
}

/// Fetches the latest revision ID of Module:Vendors/data from the Warframe wiki.
///
/// # Errors
/// Returns an error if the network request fails or the response doesn't contain a revid.
pub async fn fetch_latest_revid(client: &reqwest::Client) -> AppResult<u64> {
    let url = "https://wiki.warframe.com/api.php";
    let params = [
        ("action", "query"),
        ("prop", "revisions"),
        ("titles", "Module:Vendors/data"),
        ("rvprop", "ids"),
        ("format", "json"),
        ("formatversion", "2"),
    ];

    let response = client
        .get(url)
        .query(&params)
        .header("User-Agent", "wfm-pricer-cli")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("API request failed: {}", response.status()).into());
    }

    let json: serde_json::Value = response.json().await?;
    let revid = json
        .pointer("/query/pages/0/revisions/0/revid")
        .and_then(serde_json::Value::as_u64)
        .ok_or("Failed to extract revid from API response")?;

    Ok(revid)
}

/// Fetches the raw Lua source of Module:Vendors/data from the Warframe wiki.
/// Uses the revisions API to get the content directly.
///
/// # Errors
/// Returns an error if the network request fails, the API returns a non‑success status,
/// or the response does not contain the expected content.
pub async fn fetch_vendors_lua(client: &reqwest::Client) -> AppResult<String> {
    let url = "https://wiki.warframe.com/api.php";
    let params = [
        ("action", "query"),
        ("prop", "revisions"),
        ("titles", "Module:Vendors/data"),
        ("rvprop", "content"),
        ("rvslots", "*"),
        ("format", "json"),
        ("formatversion", "2"),
    ];

    let response = client
        .get(url)
        .query(&params)
        .header("User-Agent", "wfm-pricer-cli")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("API request failed: {}", response.status()).into());
    }

    let json: serde_json::Value = response.json().await?;
    let source = json
        .pointer("/query/pages/0/revisions/0/slots/main/content")
        .and_then(|v| v.as_str())
        .ok_or("Failed to extract content from revisions API response")?
        .to_string();

    Ok(source)
}

/// Parses the Lua source of Module:Vendors/data into a vector of `RawVendor`.
/// Returns the parsed vendors and the total number of offerings skipped due to errors.
///
/// # Errors
/// Returns a `String` error if tokenization or parsing of the Lua source fails.
pub fn parse_vendors_from_lua(source: &str) -> Result<(Vec<RawVendor>, usize), String> {
    let tokens = tokenize(source)?;
    let parsed = parse(&tokens)?;
    let top_table = parsed.as_table().ok_or("Top-level is not a table")?;
    let vendors_table = top_table
        .iter()
        .find_map(|(key, val)| {
            if let Some(LuaKey::String(s)) = key {
                if s == "Vendors" { val.as_table() } else { None }
            } else {
                None
            }
        })
        .ok_or("No 'Vendors' table found")?;
    let mut result = Vec::new();
    let mut total_skipped = 0usize;
    for (key, val) in vendors_table {
        let vendor_key = match key {
            Some(LuaKey::String(s)) => s.clone(),
            _ => return Err("Vendor key is not a string".to_string()),
        };
        let (raw, skipped) = parse_raw_vendor(vendor_key, val)?;
        result.push(raw);
        total_skipped += skipped;
    }
    Ok((result, total_skipped))
}

/// Fetches and caches vendor data if the remote revision differs from the cached one.
///
/// # Errors
/// Returns an error if the network request fails, the wiki API returns a non‑success status,
/// the Lua source cannot be parsed, or the cache files cannot be written.
pub async fn fetch_and_cache_vendors(client: &reqwest::Client) -> AppResult<()> {
    let remote_revid = fetch_latest_revid(client).await?;
    let cached_revid = read_cached_revid()?;

    if let Some(cached) = cached_revid
        && cached == remote_revid
    {
        tsprintln!(
            "Vendor data unchanged (revid {}). Skipping fetch.",
            remote_revid
        );
        return Ok(());
    }
    tsprintln!(
        "Vendor data changed (cached: {:?}, remote: {}). Fetching...",
        cached_revid,
        remote_revid
    );

    let lua_source = fetch_vendors_lua(client).await?;
    let (raw_vendors, skipped) = parse_vendors_from_lua(&lua_source)
        .map_err(|e| format!("Failed to parse vendor Lua: {e}"))?;

    write_vendor_cache(&raw_vendors)?;
    write_cached_revid(remote_revid)?;

    if skipped > 0 {
        tsprintln!(
            "Vendor cache updated ({} vendors, {} offerings parsed, {} skipped).",
            raw_vendors.len(),
            raw_vendors.iter().map(|v| v.offerings.len()).sum::<usize>(),
            skipped
        );
    } else {
        tsprintln!(
            "Vendor cache updated ({} vendors, {} offerings parsed).",
            raw_vendors.len(),
            raw_vendors.iter().map(|v| v.offerings.len()).sum::<usize>()
        );
    }
    Ok(())
}

#[cfg(test)]
mod network_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // This hits the live wiki – run with `cargo test -- --ignored`
    async fn fetch_latest_revid_returns_plausible_number() {
        let client = reqwest::Client::new();
        let revid = fetch_latest_revid(&client).await.unwrap();
        // A plausible revision ID for a popular module is > 1000000 and < 10^10
        assert!(revid > 1_000_000);
        assert!(revid < 10_000_000_000);
        tsprintln!("Latest revid: {}", revid);
    }
}
