use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use reqwest::header::USER_AGENT;
use tokio::time::{sleep, Duration};

use crate::models::{WfmStatsItem, WfmStatsResponse};

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

// ── Outlier-filtering helpers ────────────────────────────────────────────────

/// Returns the median of `values`. Returns `0.0` for an empty slice.
fn median_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    }
}

/// Filters a slice of `WfmStatsItem` references for the 90-day **closed** dataset.
///
/// A day is considered an outlier when:
/// * `wa_price > median_of_all_prices * 10`  (gross value spike), **or**
/// * `wa_price > moving_avg * 5` when `moving_avg` is present and non-zero.
///
/// If fewer than 3 entries survive filtering, the caller should fall back to the
/// unfiltered set.
fn filter_closed_outliers<'a>(
    days: &[&'a WfmStatsItem],
    price_median: f64,
) -> Vec<&'a WfmStatsItem> {
    days.iter()
        .copied()
        .filter(|d| {
            // Gross spike relative to median
            if price_median > 0.0 && d.wa_price > price_median * 10.0 {
                return false;
            }
            // Spike relative to the item's own moving average
            if let Some(ma) = d.moving_avg
                && ma > 0.0 && d.wa_price > ma * 5.0 {
                return false;
            }
            true
        })
        .collect()
}

/// Filters a slice of `WfmStatsItem` references for the **live** (order-book) dataset.
///
/// An entry is considered an outlier when `wa_price > median * 5`.
fn filter_live_outliers<'a>(
    entries: &[&'a WfmStatsItem],
    price_median: f64,
) -> Vec<&'a WfmStatsItem> {
    entries
        .iter()
        .copied()
        .filter(|d| price_median == 0.0 || d.wa_price <= price_median * 5.0)
        .collect()
}

// ── Public pricing functions ─────────────────────────────────────────────────

/// Calculates the 90-day volume-weighted average price for a target rank,
/// using outlier filtering to suppress wash-trade spikes.
///
/// Strategy:
/// 1. Compute the per-rank `median` of `wa_price` across the full 90-day window.
/// 2. Discard days where `wa_price > median * 10` **or** `wa_price > moving_avg * 5`
///    (when `moving_avg` is present and non-zero).
/// 3. If fewer than 3 days survive, revert to the unfiltered set.
/// 4. For each surviving day, use `moving_avg` as the price source when it is
///    present and non-zero; otherwise fall back to `wa_price`.
///
/// Returns `(weighted_avg_price, total_volume)`.  Both are `0` / `0.0` when no
/// matching data is found.
#[must_use]
pub fn calculate_weighted_average(
    stats: &WfmStatsResponse,
    target_rank: Option<u8>,
) -> (f64, u32) {
    let days_for_rank: Vec<&WfmStatsItem> = stats
        .payload
        .statistics_closed
        .ninety_days
        .iter()
        .filter(|d| d.mod_rank == target_rank.map(u32::from))
        .collect();

    if days_for_rank.is_empty() {
        return (0.0, 0);
    }

    // Overall median used for outlier detection
    let all_prices: Vec<f64> = days_for_rank.iter().map(|d| d.wa_price).collect();
    let price_median = median_of(&all_prices);

    let filtered = filter_closed_outliers(&days_for_rank, price_median);

    // Fall back to unfiltered when too few clean data points remain
    let working_set: &[&WfmStatsItem] = if filtered.len() < 3 {
        &days_for_rank
    } else {
        &filtered
    };

    // Volume-weighted average; prefer moving_avg as the price signal when available
    let mut total_vol: u32 = 0;
    let mut weighted_sum: f64 = 0.0;
    for day in working_set {
        let price = day
            .moving_avg
            .filter(|&ma| ma > 0.0)
            .unwrap_or(day.wa_price);
        weighted_sum += price * f64::from(day.volume);
        total_vol += day.volume;
    }

    if total_vol > 0 {
        (weighted_sum / f64::from(total_vol), total_vol)
    } else {
        // Zero-volume fallback: return the unfiltered median as a last resort
        (price_median, 0)
    }
}

/// Calculates the saturation ratio (active sell volume / latest closed volume) for a
/// target rank, filtering outlier live-stat entries before picking the latest entry.
///
/// A live entry is discarded when `wa_price > median_of_live_sells * 5`.
/// If fewer than 3 live entries survive, reverts to the unfiltered live set.
#[must_use]
pub fn calculate_saturation_ratio(
    stats: &WfmStatsResponse,
    target_rank: Option<u8>,
) -> f64 {
    // Collect all live sell entries for this rank
    let live_sells: Vec<&WfmStatsItem> = stats
        .payload
        .statistics_live
        .ninety_days
        .iter()
        .filter(|d| {
            d.mod_rank == target_rank.map(u32::from)
                && d.order_type.as_deref() == Some("sell")
        })
        .collect();

    if live_sells.is_empty() {
        return 0.0;
    }

    let live_prices: Vec<f64> = live_sells.iter().map(|d| d.wa_price).collect();
    let live_median = median_of(&live_prices);

    let filtered_live = filter_live_outliers(&live_sells, live_median);

    let working_live: &[&WfmStatsItem] = if filtered_live.len() < 3 {
        &live_sells
    } else {
        &filtered_live
    };

    // Use the chronologically latest surviving live entry
    let latest_live_sell = working_live.last().copied();

    let latest_closed = stats
        .payload
        .statistics_closed
        .ninety_days
        .iter()
        .rfind(|d| d.mod_rank == target_rank.map(u32::from));

    match (latest_live_sell, latest_closed) {
        (Some(live), Some(closed)) if closed.volume > 0 => {
            f64::from(live.volume) / f64::from(closed.volume)
        }
        _ => 0.0,
    }
}

// ── Endo helpers ───────────────────────────────────────────────────────────

#[must_use]
pub async fn derive_endo_to_plat_from_mods() -> f64 {
    // Load cached WFM items to get correct slugs
    let wfm_cache_path = crate::config::WFM_CACHE_FILE;
    let Ok(wfm_str) = fs::read_to_string(wfm_cache_path) else {
        eprintln!("[ENDO] Could not read WFM cache, using fallback rate 0.0035");
        return 0.0035;
    };
    let Ok(wfm_response) = serde_json::from_str::<crate::models::WfmV2Response>(&wfm_str) else {
        eprintln!("[ENDO] Failed to parse WFM cache, using fallback rate 0.0035");
        return 0.0035;
    };

    // Build lookup: lowercased display name -> (slug, max_rank, tags)
    let mut name_to_slug: std::collections::HashMap<String, (String, Option<u32>, Vec<String>)> = std::collections::HashMap::new();
    for item in wfm_response.data {
        let name_lower = item.i18n.en.name.to_lowercase();
        name_to_slug.insert(name_lower, (item.slug, item.max_rank, item.tags));
    }

    // Calibration candidates: (display_name, rarity, target_rank)
    // Use the same list as before, but we'll look up the slug dynamically.
    let calibration_mods = [
        ("Archon Continuity", "Legendary", 10u8,),
        ("Archon Flow", "Legendary", 10u8,),
        ("Archon Intensify", "Legendary", 10u8,),
        ("Archon Stretch", "Legendary", 10u8,),
        ("Archon Vitality", "Legendary", 10u8,),
        ("Primed Animal Instinct", "Legendary", 10u8,),
        ("Primed Charged Shell", "Legendary", 10u8,),
        ("Primed Chilling Grasp", "Legendary", 10u8,),
        ("Primed Continuity", "Legendary", 10u8,),
        ("Primed Convulsion", "Legendary", 10u8,),
        ("Primed Cryo Rounds", "Legendary", 10u8,),
        ("Primed Dual Rounds", "Legendary", 10u8,),
        ("Primed Fever Strike", "Legendary", 10u8,),
        ("Primed Firestorm", "Legendary", 10u8,),
        ("Primed Flow", "Legendary", 10u8,),
        ("Primed Fulmination", "Legendary", 10u8,),
        ("Primed Heated Charge", "Legendary", 10u8,),
        ("Primed Pistol Gambit", "Legendary", 10u8,),
        ("Primed Point Blank", "Legendary", 10u8,),
        ("Primed Pressure Point", "Legendary", 10u8,),
        ("Primed Ravage", "Legendary", 10u8,),
        ("Primed Reach", "Legendary", 10u8,),
        ("Primed Redirection", "Legendary", 10u8,),
        ("Primed Regen", "Legendary", 10u8,),
        ("Primed Target Cracker", "Legendary", 10u8,),
        ("Adaptation", "Rare", 10u8,),
        ("Bite", "Rare", 10u8,),
        ("Blind Rage", "Rare", 10u8,),
        ("Blood Rush", "Uncommon", 10u8,),
        ("Equilibrium", "Uncommon", 10u8,),
        ("Heavy Caliber", "Rare", 10u8,),
        ("Narrow Minded", "Rare", 10u8,),
        ("Preparation", "Rare", 10u8,),
        ("Rolling Guard", "Rare", 10u8,),
        ("Transient Fortitude", "Rare", 10u8,),
        ("Galvanized Acceleration", "Rare", 10u8,),
        ("Galvanized Aptitude", "Rare", 10u8,),
        ("Galvanized Chamber", "Rare", 10u8,),
        ("Galvanized Crosshairs", "Rare", 10u8,),
        ("Galvanized Diffusion", "Rare", 10u8,),
        ("Galvanized Elementalist", "Rare", 10u8,),
        ("Galvanized Hell", "Rare", 10u8,),
        ("Galvanized Reflex", "Rare", 10u8,),
        ("Galvanized Savvy", "Rare", 10u8,),
        ("Galvanized Scope", "Rare", 10u8,),
        ("Galvanized Shot", "Rare", 10u8,),
        ("Galvanized Steel", "Rare", 10u8,),
    ];

        let mut values = Vec::new();

        for (display_name, rarity, target_rank) in calibration_mods {
            let name_lower = display_name.to_lowercase();
            let Some((slug, max_rank, _tags)) = name_to_slug.get(&name_lower) else {
                println!("[ENDO] Skipping {}: not found in WFM cache", display_name);
                continue;
            };
            // Ensure the item supports the requested rank (most max_rank is 10)
            if max_rank.map_or(false, |mr| mr < target_rank.into()) {
                println!("[ENDO] Skipping {}: max_rank {} < required {}", display_name, max_rank.unwrap_or(0), target_rank);
                continue;
            }

            let stats = match fetch_statistics(slug).await {
                Ok(s) => s,
                Err(e) => {
                    println!("[ENDO] Skipping {} ({}): fetch failed: {}", display_name, slug, e);
                    continue;
                }
            };

            let (rank0_price, _vol_0) = calculate_weighted_average(&stats, Some(0));
            let (ranked_price, _vol_r) = calculate_weighted_average(&stats, Some(target_rank));

            let endo_cost = calculate_mod_upgrade_endo(rarity, target_rank.into());
            if endo_cost == 0 {
                continue;
            }

            if ranked_price <= rank0_price {
                // No price increase – skip
                continue;
            }

            let delta = ranked_price - rank0_price;
            let rate = delta / f64::from(endo_cost);

            // Optional: keep a single line for debugging (comment out for production)
            // println!("[ENDO] {}: Δ={:.2}p, endo={}, rate={:.6}", display_name, delta, endo_cost, rate);

            values.push(rate);
        }

        if values.is_empty() {
            eprintln!("[ENDO] No valid calibration mods, using fallback rate 0.0035");
            return 0.0035;
        }

        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = values.len() / 2;
        let median = if values.len() % 2 == 0 {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        };

        median
    }

/// Dynamic Endo yield for Ayatan items.
#[must_use]
pub fn get_ayatan_endo_yield(slug: &str) -> Option<u32> {
    match slug {
        "ayatan_cyan_star"          => Some(80),
        "ayatan_amber_star"         => Some(100),
        "ayatan_anasa_sculpture"    => Some(3450),
        "ayatan_ayr_sculpture"      => Some(1425),
        "ayatan_orta_sculpture"     => Some(2700),
        "ayatan_sah_sculpture"      => Some(1500),
        "ayatan_valana_sculpture"   => Some(1575),
        "ayatan_vaya_sculpture"     => Some(1800),
        "ayatan_piv_sculpture"      => Some(1725),
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

// ── Arcane / Mod helpers ─────────────────────────────────────────────────────

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
        "Uncommon"  => 20,
        "Common"    => 10,
        _           => 30,
    }
}

/// Mod rank helpers to calculate WFM trade tax.
#[must_use]
pub fn get_mod_trade_tax(rarity: &str) -> u32 {
    match rarity {
        "Legendary" => 1_000_000,
        "Uncommon"  =>     4_000,
        "Common"    =>     2_000,
        _           =>     8_000,
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
