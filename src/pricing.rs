use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;
use reqwest::header::USER_AGENT;
use tokio::time::{sleep, Duration};

use crate::models::{WfmStatsItem, WfmStatsResponse};
use crate::{tseprintln, tsprintln};

pub const STATS_CACHE_DIR: &str = "cache/statistics";

/// Number of attempts (including the first) made per slug before giving up.
const STATS_MAX_ATTEMPTS: u32 = 4;
/// Per-request timeout. Without this, a stalled connection to WFM (or an intermediary
/// proxy that accepts the connection but never responds) hangs on the OS-level TCP
/// timeout instead of failing fast — this is what was showing up as requests randomly
/// taking 70-90s instead of ~400ms.
const STATS_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const STATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Shared, connection-pooling HTTP client for statistics requests. Previously
/// `fetch_statistics` built a brand-new `reqwest::Client` on every single call, which
/// discarded keep-alive/connection-pooling (paying a fresh TCP+TLS handshake per request
/// across the ~900+ requests in a session) and had no timeout configured at all.
fn stats_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(STATS_REQUEST_TIMEOUT)
            .connect_timeout(STATS_CONNECT_TIMEOUT)
            .build()
            .expect("failed to build reqwest client for statistics requests")
    })
}

// ── fetch_statistics ────────────────────────────────────────────────────────

/// Fetches statistics for a given item slug from WFM.
/// Respects a 400ms rate limit and caches the response locally to avoid repeated requests.
/// Retries transient failures (connection errors, timeouts, 429, and 5xx responses) with
/// exponential backoff before giving up; non-transient 4xx responses fail immediately.
///
/// # Errors
/// Returns an error if file operations fail, JSON parsing fails, or all retry attempts
/// are exhausted without a successful response.
pub async fn fetch_statistics(slug: &str) -> Result<WfmStatsResponse, Box<dyn Error>> {
    fs::create_dir_all(STATS_CACHE_DIR)?;
    let cache_path = PathBuf::from(STATS_CACHE_DIR).join(format!("{slug}.json"));

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

    sleep(Duration::from_millis(400)).await;

    tsprintln!("Fetching market statistics for '{slug}'...");
    let url = format!("https://api.warframe.market/v1/items/{slug}/statistics");
    let client = stats_http_client();

    let mut last_err = String::new();

    for attempt in 1..=STATS_MAX_ATTEMPTS {
        let send_result = client
            .get(&url)
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await;

        match send_result {
            Ok(response) if response.status().is_success() => {
                let stats: WfmStatsResponse = response.json().await?;
                if let Ok(serialized) = serde_json::to_string_pretty(&stats) {
                    let _ = fs::write(&cache_path, serialized);
                }
                return Ok(stats);
            }
            Ok(response) => {
                let status = response.status();
                last_err = format!("{status}");
                // 4xx other than 429 (rate limited) means the request itself is wrong
                // (bad slug, etc.) — retrying won't help, so fail fast.
                if status.is_client_error() && status.as_u16() != 429 {
                    return Err(format!("Failed to fetch stats for {slug}: {status}").into());
                }
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }

        if attempt < STATS_MAX_ATTEMPTS {
            let backoff = Duration::from_secs(2u64.pow(attempt - 1)); // 1s, 2s, 4s
            tseprintln!(
                "  '{slug}' attempt {attempt}/{STATS_MAX_ATTEMPTS} failed ({last_err}), retrying in {}s...",
                backoff.as_secs()
            );
            sleep(backoff).await;
        }
    }

    Err(format!(
        "Failed to fetch stats for {slug} after {STATS_MAX_ATTEMPTS} attempts: {last_err}"
    )
    .into())
}

// ── Outlier-filtering helpers ────────────────────────────────────────────────

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

fn filter_closed_outliers<'a>(
    days: &[&'a WfmStatsItem],
    price_median: f64,
) -> Vec<&'a WfmStatsItem> {
    days.iter()
        .copied()
        .filter(|d| {
            if price_median > 0.0 && d.wa_price > price_median * 10.0 {
                return false;
            }
            if let Some(ma) = d.moving_avg
                && ma > 0.0 && d.wa_price > ma * 5.0 {
                return false;
            }
            true
        })
        .collect()
}

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

/// Total volume and number of distinct trading days within the most recent `window_days`
/// *calendar* days, anchored to the latest matching entry's own date — not array position,
/// and not wall‑clock "now" (so this is reproducible against cached/fixture data).
#[must_use]
pub fn recent_volume(
    stats: &WfmStatsResponse,
    target_rank: Option<u8>,
    window_days: i64,
) -> (u32, u32) {
    use chrono::{DateTime, Duration};
    let days_for_rank: Vec<&WfmStatsItem> = stats
        .payload
        .statistics_closed
        .ninety_days
        .iter()
        .filter(|d| d.mod_rank == target_rank.map(u32::from))
        .collect();

    let Some(latest_dt) = days_for_rank
        .iter()
        .filter_map(|d| DateTime::parse_from_rfc3339(&d.datetime).ok())
        .max()
    else {
        return (0, 0);
    };

    let cutoff = latest_dt - Duration::days(window_days.saturating_sub(1));

    let mut volume = 0u32;
    let mut trading_days = 0u32;
    for d in &days_for_rank {
        if let Ok(dt) = DateTime::parse_from_rfc3339(&d.datetime)
            && dt >= cutoff
        {
            volume += d.volume;
            trading_days += 1;
        }
    }
    (volume, trading_days)
}

/// Calculates the 90-day volume-weighted average price for a target rank,
/// using outlier filtering to suppress wash-trade spikes.
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

    let all_prices: Vec<f64> = days_for_rank.iter().map(|d| d.wa_price).collect();
    let price_median = median_of(&all_prices);

    let filtered = filter_closed_outliers(&days_for_rank, price_median);

    let working_set: &[&WfmStatsItem] = if filtered.len() < 3 {
        &days_for_rank
    } else {
        &filtered
    };

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
        (price_median, 0)
    }
}

/// Calculates the saturation ratio (active sell volume / latest closed volume) for a
/// target rank, filtering outlier live-stat entries before picking the latest entry.
#[must_use]
pub fn calculate_saturation_ratio(
    stats: &WfmStatsResponse,
    target_rank: Option<u8>,
) -> f64 {
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

async fn collect_calibration_rates() -> Vec<f64> {
    let wfm_cache_path = crate::config::WFM_CACHE_FILE;
    let Ok(wfm_str) = fs::read_to_string(wfm_cache_path) else {
        tseprintln!("[ENDO] Could not read WFM cache, using fallback rate 0.0035");
        return Vec::new();
    };
    let Ok(wfm_response) = serde_json::from_str::<crate::models::WfmV2Response>(&wfm_str) else {
        tseprintln!("[ENDO] Failed to parse WFM cache, using fallback rate 0.0035");
        return Vec::new();
    };

    let mut name_to_slug = HashMap::new();
    for item in wfm_response.data {
        let name_lower = item.i18n.en.name.to_lowercase();
        name_to_slug.insert(name_lower, (item.slug, item.max_rank, item.tags));
    }

    let calibration_mods = [
        ("Archon Continuity", "Legendary", 10u8),
        ("Archon Flow", "Legendary", 10u8),
        ("Archon Intensify", "Legendary", 10u8),
        ("Archon Stretch", "Legendary", 10u8),
        ("Archon Vitality", "Legendary", 10u8),
        ("Primed Animal Instinct", "Legendary", 10u8),
        ("Primed Charged Shell", "Legendary", 10u8),
        ("Primed Chilling Grasp", "Legendary", 10u8),
        ("Primed Continuity", "Legendary", 10u8),
        ("Primed Convulsion", "Legendary", 10u8),
        ("Primed Cryo Rounds", "Legendary", 10u8),
        ("Primed Dual Rounds", "Legendary", 10u8),
        ("Primed Fever Strike", "Legendary", 10u8),
        ("Primed Firestorm", "Legendary", 10u8),
        ("Primed Flow", "Legendary", 10u8),
        ("Primed Fulmination", "Legendary", 10u8),
        ("Primed Heated Charge", "Legendary", 10u8),
        ("Primed Pistol Gambit", "Legendary", 10u8),
        ("Primed Point Blank", "Legendary", 10u8),
        ("Primed Pressure Point", "Legendary", 10u8),
        ("Primed Ravage", "Legendary", 10u8),
        ("Primed Reach", "Legendary", 10u8),
        ("Primed Redirection", "Legendary", 10u8),
        ("Primed Regen", "Legendary", 10u8),
        ("Primed Target Cracker", "Legendary", 10u8),
        ("Adaptation", "Rare", 10u8),
        ("Bite", "Rare", 10u8),
        ("Blind Rage", "Rare", 10u8),
        ("Blood Rush", "Uncommon", 10u8),
        ("Equilibrium", "Uncommon", 10u8),
        ("Heavy Caliber", "Rare", 10u8),
        ("Narrow Minded", "Rare", 10u8),
        ("Preparation", "Rare", 10u8),
        ("Rolling Guard", "Rare", 10u8),
        ("Transient Fortitude", "Rare", 10u8),
        ("Galvanized Acceleration", "Rare", 10u8),
        ("Galvanized Aptitude", "Rare", 10u8),
        ("Galvanized Chamber", "Rare", 10u8),
        ("Galvanized Crosshairs", "Rare", 10u8),
        ("Galvanized Diffusion", "Rare", 10u8),
        ("Galvanized Elementalist", "Rare", 10u8),
        ("Galvanized Hell", "Rare", 10u8),
        ("Galvanized Reflex", "Rare", 10u8),
        ("Galvanized Savvy", "Rare", 10u8),
        ("Galvanized Scope", "Rare", 10u8),
        ("Galvanized Shot", "Rare", 10u8),
        ("Galvanized Steel", "Rare", 10u8),
    ];

    let mut rates = Vec::new();
    for (display_name, rarity, target_rank) in calibration_mods {
        let name_lower = display_name.to_lowercase();
        let Some((slug, max_rank, _tags)) = name_to_slug.get(&name_lower) else {
            tsprintln!("[ENDO] Skipping {display_name}: not found in WFM cache");
            continue;
        };
        if max_rank.is_some_and(|mr| mr < target_rank.into()) {
            tsprintln!("[ENDO] Skipping {display_name}: max_rank {} < required {target_rank}", max_rank.unwrap_or(0));
            continue;
        }

        let stats = match fetch_statistics(slug).await {
            Ok(s) => s,
            Err(e) => {
                tsprintln!("[ENDO] Skipping {display_name} ({slug}): fetch failed: {e}");
                continue;
            }
        };

        let (rank0_price, _) = calculate_weighted_average(&stats, Some(0));
        let (ranked_price, _) = calculate_weighted_average(&stats, Some(target_rank));
        let endo_cost = calculate_mod_upgrade_endo(rarity, target_rank.into());
        if endo_cost == 0 || ranked_price <= rank0_price {
            continue;
        }
        let delta = ranked_price - rank0_price;
        rates.push(delta / f64::from(endo_cost));
    }
    rates
}

/// Derives the Endo‑to‑Platinum exchange rate from a set of calibration mods.
#[must_use]
pub async fn derive_endo_to_plat_from_mods() -> f64 {
    let rates = collect_calibration_rates().await;
    if rates.is_empty() {
        tseprintln!("[ENDO] No valid calibration mods, using fallback rate 0.0035");
        return 0.0035;
    }
    median_of(&rates)
}

// ── Ayatan and other helpers ──────────────────────────────────────────────

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
        0.0035
    }
}

// ── Arcane / Mod helpers ─────────────────────────────────────────────────────

/// Helper to get the number of raw copy equivalents for an Arcane rank.
#[must_use]
pub fn get_arcane_rank_copies(rank: u32) -> u32 {
    ((rank + 1) * (rank + 2)) / 2
}

#[must_use]
pub fn is_antique(_slug: &str, game_ref: &str) -> bool {
    game_ref.contains("/Antiques/")
}

#[must_use]
pub fn get_rarity_multiplier(rarity: &str) -> u32 {
    match rarity.to_lowercase().as_str() {
        "common"    => 1,
        "uncommon" | "peculiar"  => 2,
        "legendary" => 4,
        _           => 3,
    }
}

#[must_use]
pub fn get_fusion_cost_from_zero(rarity: &str, target_rank: u32, is_antique: bool) -> u32 {
    if target_rank == 0 { return 0; }

    let base_multiplier = if is_antique { 160 } else { 10 };
    let rarity_num = get_rarity_multiplier(rarity);

    base_multiplier * rarity_num * (2u32.pow(target_rank) - 1)
}

#[must_use]
pub fn get_mod_base_endo(rarity: &str) -> u32 {
    match rarity {
        "Legendary" => 40,
        "Uncommon"  => 20,
        "Common"    => 10,
        _           => 30,
    }
}

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
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn calculate_mod_upgrade_credits(endo_cost: u32) -> u32 {
    (f64::from(endo_cost) * 48.25) as u32
}

#[cfg(test)]
mod endo_upgrade_tests {
    use super::*;

    #[test]
    fn endo_to_max_is_nonzero_when_current_rank_below_max() {
        let cost_to_current = get_fusion_cost_from_zero("Rare", 3, false);
        let cost_to_max = get_fusion_cost_from_zero("Rare", 10, false);
        let endo_to_max = cost_to_max.saturating_sub(cost_to_current);
        assert!(endo_to_max > 0, "a rank‑3‑of‑10 mod must have nonzero endo cost remaining");
    }
}

#[cfg(test)]
mod recent_volume_tests {
    use super::*;
    use std::fs;

    fn load_fixture(name: &str) -> WfmStatsResponse {
        let path = format!("tests/fixtures/test_statistics/{name}.json");
        let raw = fs::read_to_string(&path).expect("fixture missing — see Task 0.1");
        serde_json::from_str(&raw).expect("fixture failed to parse")
    }

    #[test]
    fn voruna_recent_volume_matches_known_value_not_stale_value() {
        let stats = load_fixture("voruna_prime_set");
        let (volume, trading_days) = recent_volume(&stats, None, 30);
        assert_eq!(trading_days, 30);
        // Known correct value from Task 0.1 manifest: ~13,790
        assert!(
            (12_000..=15_500).contains(&volume),
            "expected recent volume near 13790, got {volume}"
        );
    }
}
