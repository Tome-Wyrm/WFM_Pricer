use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use reqwest::header::USER_AGENT;
use tokio::time::{sleep, Duration};

use crate::models::WfmStatsResponse;

pub const STATS_CACHE_DIR: &str = "cache/statistics";

/// Fetches statistics for a given item slug from WFM.
/// Respects a 400ms rate limit and caches the response locally to avoid repeated requests.
///
/// # Errors
/// Returns an error if file operations fail, JSON parsing fails, or network requests fail.
pub async fn fetch_statistics(slug: &str) -> Result<WfmStatsResponse, Box<dyn Error>> {
    fs::create_dir_all(STATS_CACHE_DIR)?;
    let cache_path = PathBuf::from(STATS_CACHE_DIR).join(format!("{slug}.json"));

    // 1. Check if cache exists and is fresh (less than 24 hours old)
    if cache_path.exists()
        && let Ok(metadata) = fs::metadata(&cache_path)
        && let Ok(modified) = metadata.modified()
        && let Ok(duration) = SystemTime::now().duration_since(modified)
        && duration < Duration::from_secs(24 * 60 * 60)
        && let Ok(content) = fs::read_to_string(&cache_path)
        && let Ok(stats) = serde_json::from_str::<WfmStatsResponse>(&content)
    {
        return Ok(stats);
    }

    // 2. Fetch from WFM API (with rate limit delay)
    sleep(Duration::from_millis(400)).await;

    println!("Fetching market statistics for '{slug}'...");
    let client = reqwest::Client::new();
    let url = format!("https://api.warframe.market/v1/items/{slug}/statistics");
    let response = client
        .get(&url)
        .header(USER_AGENT, "wfm-pricer-cli")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("Failed to fetch stats for {}: {}", slug, response.status()).into());
    }

    let stats: WfmStatsResponse = response.json().await?;
    
    // Save to cache
    if let Ok(serialized) = serde_json::to_string_pretty(&stats) {
        let _ = fs::write(&cache_path, serialized);
    }

    Ok(stats)
}

/// Calculates the 90-day volume-weighted average price for a target rank.
#[must_use]
pub fn calculate_weighted_average(
    stats: &WfmStatsResponse,
    target_rank: Option<u32>,
) -> (f64, u32) {
    let mut total_vol = 0;
    let mut weighted_sum = 0.0;

    for day in &stats.payload.statistics_closed.ninety_days {
        if day.mod_rank == target_rank {
            weighted_sum += day.wa_price * f64::from(day.volume);
            total_vol += day.volume;
        }
    }

    if total_vol > 0 {
        (weighted_sum / f64::from(total_vol), total_vol)
    } else {
        // Fallback to latest matching day
        if let Some(latest) = stats
            .payload
            .statistics_closed
            .ninety_days
            .iter()
            .rfind(|d| d.mod_rank == target_rank)
        {
            (latest.wa_price, latest.volume)
        } else {
            (0.0, 0)
        }
    }
}

/// Calculates the saturation ratio (active sell volume / closed trade volume) for a target rank on the latest day.
#[must_use]
pub fn calculate_saturation_ratio(
    stats: &WfmStatsResponse,
    target_rank: Option<u32>,
) -> f64 {
    let latest_live_sell = stats
        .payload
        .statistics_live
        .ninety_days
        .iter()
        .rfind(|d| {
            d.mod_rank == target_rank
                && d.order_type.as_deref() == Some("sell")
        });

    let latest_closed = stats
        .payload
        .statistics_closed
        .ninety_days
        .iter()
        .rfind(|d| d.mod_rank == target_rank);

    match (latest_live_sell, latest_closed) {
        (Some(live), Some(closed)) => {
            if closed.volume > 0 {
                f64::from(live.volume) / f64::from(closed.volume)
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// Dynamic Endo yield for Ayatan items.
#[must_use]
pub fn get_ayatan_endo_yield(slug: &str) -> Option<u32> {
    match slug {
        "ayatan_cyan_star" => Some(80),
        "ayatan_amber_star" => Some(100),
        "ayatan_anasa_sculpture" => Some(3450),
        "ayatan_ayr_sculpture" => Some(1425),
        "ayatan_orta_sculpture" => Some(2700),
        "ayatan_sah_sculpture" => Some(1500),
        "ayatan_valana_sculpture" => Some(1575),
        "ayatan_vaya_sculpture" => Some(1800),
        "ayatan_piv_sculpture" => Some(1725),
        "ayatan_hemakara_sculpture" => Some(3200),
        _ => None,
    }
}

/// Derives the plat-per-endo exchange rate dynamically from priced Ayatan sculptures and stars.
#[must_use]
pub fn derive_endo_to_plat_rate<S: ::std::hash::BuildHasher>(prices: &HashMap<String, f64, S>) -> f64 {
    let mut sum = 0.0;
    let mut count = 0;

    for (slug, price) in prices {
        if let Some(yield_endo) = get_ayatan_endo_yield(slug)
            && *price > 0.0
        {
                sum += price / f64::from(yield_endo);
                count += 1;
        }
    }

    if count > 0 {
        sum / f64::from(count)
    } else {
        // Safe default: 0.0035 plat per endo
        0.0035
    }
}

/// Helper to get the number of raw copy equivalents for an Arcane rank.
#[must_use]
pub fn get_arcane_rank_copies(rank: u32) -> u32 {
    ((rank + 1) * (rank + 2)) / 2
}

/// Mod rank helpers to calculate upgrade endo cost.
#[must_use]
pub fn get_mod_base_endo(rarity: &str) -> u32 {
    match rarity {
        "Legendary" => 40,
        "Uncommon" => 20,
        "Common" => 10,
        _ => 30,
    }
}

/// Mod rank helpers to calculate WFM trade tax.
#[must_use]
pub fn get_mod_trade_tax(rarity: &str) -> u32 {
    match rarity {
        "Legendary" => 1_000_000,
        "Uncommon" => 4_000,
        "Common" => 2_000,
        _ => 8_000,
    }
}

#[must_use]
pub fn calculate_mod_upgrade_endo(rarity: &str, target_rank: u32) -> u32 {
    let base = get_mod_base_endo(rarity);
    base * (2u32.pow(target_rank) - 1)
}

#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Intentional truncation for credit calculation
pub fn calculate_mod_upgrade_credits(endo_cost: u32) -> u32 {
    (f64::from(endo_cost) * 48.25) as u32
}
