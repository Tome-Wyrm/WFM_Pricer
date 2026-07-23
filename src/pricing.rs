//! Pricing domain logic: WFM market-statistics fetch/caching, weighted-average and
//! saturation-ratio calculations, Endo/fusion-cost math, and set-aggregation/upgrade-
//! suggestion logic (moved here from `cli::aggregation` — see git history — since none of it
//! is presentation code). Consumed by `cli::pricing`/`cli::sell` for the interactive sell
//! flow and by `cli::primed_mods`/`cli::check_sets` for their reports; this module itself
//! has no knowledge of the CLI.

use crate::AppResult;
use reqwest::header::USER_AGENT;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;
use tokio::time::{Duration, sleep};

use crate::config::MIN_DAILY_VOLUME;
use crate::mapping::{BuildParentMap, BuildRequirements, resolve_set_item};
use crate::models::{MappedItem, WfcdItem, WfmItem, WfmStatsItem, WfmStatsResponse};
use crate::repository::{StatisticsRepository, StatisticsRepositoryJson};
use crate::wfm_client::{ListingKey, Order as OwnedOrder};
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
pub async fn fetch_statistics(slug: &str) -> AppResult<WfmStatsResponse> {
    fs::create_dir_all(STATS_CACHE_DIR)?;
    let cache_path = PathBuf::from(STATS_CACHE_DIR).join(format!("{slug}.json"));

    // Phase 2 cleanup: the TTL check still needs the file's own mtime directly
    // (not part of the StatisticsRepository trait), but the actual cached-value
    // read now goes through StatisticsRepositoryJson instead of a second
    // hand-rolled read_to_string + from_str here.
    if cache_path.exists()
        && let Ok(metadata) = fs::metadata(&cache_path)
        && let Ok(modified) = metadata.modified()
        && let Ok(duration) = SystemTime::now().duration_since(modified)
        && duration < Duration::from_secs(24 * 60 * 60)
    {
        let stats_repo: StatisticsRepositoryJson<WfmStatsResponse> =
            StatisticsRepositoryJson::open_default();
        if let Ok(stats) = stats_repo.get(&slug.to_string()) {
            return Ok(stats);
        }
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
                let mut stats_repo: StatisticsRepositoryJson<WfmStatsResponse> =
                    StatisticsRepositoryJson::open_default();
                // Same as before: caching is best-effort, a write failure
                // doesn't fail the fetch itself.
                let _ = stats_repo.upsert_ref(slug, &stats);
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

    Err(
        format!("Failed to fetch stats for {slug} after {STATS_MAX_ATTEMPTS} attempts: {last_err}")
            .into(),
    )
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
                && ma > 0.0
                && d.wa_price > ma * 5.0
            {
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

/// Resolves the effective rank filter to apply against `stats`, correcting for callers
/// that guess a specific numeric rank (e.g. `Some(0)` for "vendor sells the unranked
/// copy", or a `maxRank` pulled from the item cache) when the item is actually one WFM
/// tracks with a `null` rank on *every* statistics row instead of numeric ranks.
/// Confirmed against a real 90-day statistics dump for "Peculiar Audience": all 88 rows
/// have `rank: null`, so any caller requesting `Some(0)` or another specific rank for it
/// would filter out 100% of real data and silently report a 0.0 price / 0 volume.
///
/// Falls back to `None` only when the item has *no* numeric-rank rows anywhere — i.e.
/// it's genuinely not rank-tracked by WFM. It must NOT fall back just because the
/// specific requested rank happens to have zero matches: a real ranked mod (e.g. Primed
/// Continuity) can legitimately have thin or zero rank-0 trading in a given window, and
/// falling back to `None` in that case doesn't mean "unranked" — it accidentally picks
/// up whatever stray null-rank rows exist for the item (mixed in for other reasons,
/// e.g. legacy/unspecified-rank orders), which for popular items skew heavily toward
/// max-rank-like prices and volumes. That was the bug behind `--min-rank` silently
/// reporting max-rank numbers for real ranked mods.
fn resolve_target_rank_in<'a, I: Iterator<Item = &'a WfmStatsItem>>(
    rows: I,
    requested: Option<u8>,
) -> Option<u8> {
    let r = requested?;
    let mut has_requested = false;
    let mut has_any_numeric_rank = false;
    for row in rows {
        if row.rank == Some(u32::from(r)) {
            has_requested = true;
        }
        if row.rank.is_some() {
            has_any_numeric_rank = true;
        }
    }
    if has_requested || has_any_numeric_rank {
        requested
    } else {
        None
    }
}

fn resolve_target_rank(stats: &WfmStatsResponse, requested: Option<u8>) -> Option<u8> {
    resolve_target_rank_in(
        stats.payload.statistics_closed.ninety_days.iter(),
        requested,
    )
}

fn resolve_target_rank_live(stats: &WfmStatsResponse, requested: Option<u8>) -> Option<u8> {
    resolve_target_rank_in(stats.payload.statistics_live.ninety_days.iter(), requested)
}

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
    let target_rank = resolve_target_rank(stats, target_rank);
    let days_for_rank: Vec<&WfmStatsItem> = stats
        .payload
        .statistics_closed
        .ninety_days
        .iter()
        .filter(|d| d.rank == target_rank.map(u32::from))
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
pub fn calculate_weighted_average(stats: &WfmStatsResponse, target_rank: Option<u8>) -> (f64, u32) {
    let target_rank = resolve_target_rank(stats, target_rank);
    let days_for_rank: Vec<&WfmStatsItem> = stats
        .payload
        .statistics_closed
        .ninety_days
        .iter()
        .filter(|d| d.rank == target_rank.map(u32::from))
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
pub fn calculate_saturation_ratio(stats: &WfmStatsResponse, target_rank: Option<u8>) -> f64 {
    let target_rank = resolve_target_rank_live(stats, target_rank);
    let live_sells: Vec<&WfmStatsItem> = stats
        .payload
        .statistics_live
        .ninety_days
        .iter()
        .filter(|d| d.rank == target_rank.map(u32::from) && d.order_type.as_deref() == Some("sell"))
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
        .rfind(|d| d.rank == target_rank.map(u32::from));

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
            tsprintln!(
                "[ENDO] Skipping {display_name}: max_rank {} < required {target_rank}",
                max_rank.unwrap_or(0)
            );
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
pub fn derive_endo_to_plat_rate<S: ::std::hash::BuildHasher>(
    prices: &HashMap<String, f64, S>,
) -> f64 {
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
        "common" => 1,
        "uncommon" | "peculiar" => 2,
        "legendary" => 4,
        _ => 3,
    }
}

#[must_use]
pub fn get_fusion_cost_from_zero(rarity: &str, target_rank: u32, is_antique: bool) -> u32 {
    if target_rank == 0 {
        return 0;
    }

    let base_multiplier = if is_antique { 160 } else { 10 };
    let rarity_num = get_rarity_multiplier(rarity);

    base_multiplier * rarity_num * (2u32.pow(target_rank) - 1)
}

#[must_use]
pub fn get_mod_base_endo(rarity: &str) -> u32 {
    match rarity {
        "Legendary" => 40,
        "Uncommon" => 20,
        "Common" => 10,
        _ => 30,
    }
}

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
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn calculate_mod_upgrade_credits(endo_cost: u32) -> u32 {
    (f64::from(endo_cost) * 48.25) as u32
}

// ── Set aggregation & upgrade suggestions ──────────────────────────────
// Moved from cli::aggregation (Phase 1 architecture cleanup): this is pure
// pricing/build-aggregation domain logic with no CLI presentation code, so
// it belongs here rather than under src/cli/.
pub(crate) fn aggregate_sets_with_prices(
    candidates: Vec<MappedItem>,
    parent_map: &BuildParentMap,
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    prices: &HashMap<String, f64>,
) -> Vec<MappedItem> {
    // Separate parts that belong to a build from everything else
    let mut part_items = Vec::new();
    let mut other_items = Vec::new();

    for item in candidates {
        if parent_map.contains_key(&item.game_ref) {
            part_items.push(item);
        } else {
            other_items.push(item);
        }
    }

    // Build component quantity map: game_ref -> (total_qty, example_item)
    let mut component_qty: HashMap<String, (u32, MappedItem)> = HashMap::new();
    for item in &part_items {
        let key = item.game_ref.clone();
        let (qty, _) = component_qty.entry(key).or_insert((0, item.clone()));
        *qty += item.quantity;
    }

    let mut set_items = Vec::new();
    let mut consumed: HashMap<String, u32> = HashMap::new();

    // Process each build
    for (build_unique, recipe) in requirements {
        let Some(wfcd_item) = wfcd_by_ref.get(build_unique) else {
            continue;
        };
        let Some(set_item) = resolve_set_item(&wfcd_item.name, wfm_by_name) else {
            continue;
        };

        // Determine possible sets ignoring guard
        let mut possible_sets = u32::MAX;
        for (comp_unique, required_qty) in recipe {
            if let Some((qty, _)) = component_qty.get(comp_unique) {
                let avail = qty.saturating_sub(consumed.get(comp_unique).copied().unwrap_or(0));
                possible_sets = possible_sets.min(avail / required_qty);
                if possible_sets == 0 {
                    break;
                }
            } else {
                possible_sets = 0;
                break;
            }
        }

        if possible_sets == 0 {
            continue;
        }

        let set_price = *prices.get(&set_item.slug).unwrap_or(&0.0);
        if set_price <= 0.0 {
            continue;
        }

        // Form sets
        let sets_to_form = possible_sets;
        let set_mapped = MappedItem {
            id: set_item.id.clone(),
            slug: set_item.slug.clone(),
            name: set_item.i18n.en.name.clone(),
            quantity: sets_to_form,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: build_unique.clone(),
            subtypes: set_item.subtypes.clone(),
            owned_subtype: None,
            bulk_tradable: set_item.bulk_tradable,
        };
        set_items.push(set_mapped);

        // Record consumption
        for (comp_unique, required_qty) in recipe {
            *consumed.entry(comp_unique.clone()).or_insert(0) += required_qty * sets_to_form;
        }
    }

    // Build final list: sets + leftovers + other_items
    let mut result = set_items;

    // `component_qty` is a HashMap (fine for the .get() lookups above — those are point
    // lookups, not iteration), but this loop iterates it and pushes straight into `result`,
    // so a HashMap's randomized per-process iteration order would make the displayed
    // leftover-part ordering non-deterministic across runs on identical input. Sorting the
    // keys first is cheap here (this runs once per CLI invocation over a small candidate
    // list) and avoids widening `component_qty`'s type just for this one call site.
    let mut leftover_keys: Vec<String> = component_qty.keys().cloned().collect();
    leftover_keys.sort();

    for comp_unique in leftover_keys {
        let (total_qty, comp_item_template) = &component_qty[&comp_unique];
        let used = consumed.get(&comp_unique).copied().unwrap_or(0);
        let leftover = total_qty.saturating_sub(used);
        if leftover > 0 {
            // Same "worth a listing slot" heuristic that filter_candidates used to apply
            // pre-aggregation. Applied here — after consumption is accounted for — it only
            // prunes genuinely orphaned components (no completable set exists for them, or
            // they weren't needed for the sets we did form), rather than the raw parts an
            // in-progress set still needs, which must reach aggregation first.
            let name_lower = comp_item_template.name.to_lowercase();
            let worth_reviewing = name_lower.contains("prime")
                || name_lower.contains("set")
                || name_lower.contains("blueprint");
            if !worth_reviewing {
                continue;
            }
            let mut leftover_item = comp_item_template.clone();
            leftover_item.quantity = leftover;
            result.push(leftover_item);
        }
    }

    result.extend(other_items);
    result
}

pub(crate) fn filter_candidates(
    mapped_items: Vec<MappedItem>,
    parent_map: &BuildParentMap,
) -> Vec<MappedItem> {
    tsprintln!("Filtering high-value candidates for trade review...");
    mapped_items
        .into_iter()
        .filter(|item| {
            if item.is_arcane || item.is_ayatan {
                return true;
            }
            if item.is_mod {
                return item.max_rank.is_some();
            }
            // Known build components (Barrels, Receivers, Stocks, Chassis, Systems,
            // Neuroptics, Hilts, Blades, Links, Guards, Gauntlets, ...) must always reach
            // aggregate_sets_with_prices intact, even though their display names usually
            // contain none of "prime"/"set"/"blueprint" (only non-Prime part names lack
            // "prime"; only the blueprint itself contains "blueprint"). Dropping them here
            // silently starved the set-aggregator of exactly the pieces it needs to detect a
            // completed set — e.g. a Barrel/Receiver pair with no "prime" or "blueprint" in
            // their names would vanish before aggregation ever saw them, so a genuinely
            // complete set never got formed and its Blueprint sat as a 100% "leftover"
            // instead of being partially consumed. Post-aggregation, true junk components
            // (parts of builds we'll never complete) still get pruned as unconsumed leftovers
            // further down the pipeline via the name filter below — this only protects them
            // from being discarded *before* aggregation gets a chance to use them.
            if parent_map.contains_key(&item.game_ref) {
                return true;
            }
            let name_lower = item.name.to_lowercase();
            name_lower.contains("prime")
                || name_lower.contains("set")
                || name_lower.contains("blueprint")
        })
        .collect()
}

/// Computes a mod's upgrade suggestion (price delta, endo cost to max, and a ranking score) if
/// leveling it to `max_rank` would be profitable. Returns `None` if it's already maxed, there's
/// no endo left to spend, or leveling wouldn't raise the price enough to be worth it.
///
/// Pulled out as a pure function specifically so this is unit-testable without a live
/// `build_priced_candidates` pipeline — see `upgrade_suggestion_tests` below, including a
/// regression test for the rank-0 case that was previously, silently, never suggested.
pub(crate) fn upgrade_suggestion(
    rarity: &str,
    current_rank: u32,
    max_rank: u32,
    is_antique: bool,
    wa_price: f64,
    max_price: f64,
    vol_30d: u32,
) -> Option<(f64, u32, f64)> {
    if current_rank >= max_rank {
        return None;
    }
    let endo_cost = get_fusion_cost_from_zero(rarity, current_rank, is_antique);
    let endo_to_max =
        get_fusion_cost_from_zero(rarity, max_rank, is_antique).saturating_sub(endo_cost);
    if endo_to_max == 0 {
        return None;
    }
    let delta = max_price - wa_price;
    if delta <= 0.0 {
        return None;
    }
    let upgrade_score = (delta / f64::from(endo_to_max)) * (1.0 + f64::from(vol_30d)).ln();
    Some((delta, endo_to_max, upgrade_score))
}

/// Abstraction over how `build_priced_candidates` retrieves market statistics for a slug.
/// Exists purely so tests can supply fixture data instead of making a real network call —
/// production code always uses `LiveStatsSource`, which just delegates to `fetch_statistics`.
pub(crate) trait StatsSource {
    async fn fetch(&self, slug: &str) -> AppResult<WfmStatsResponse>;
}

#[cfg(test)]
mod endo_upgrade_tests {
    use super::*;

    #[test]
    fn endo_to_max_is_nonzero_when_current_rank_below_max() {
        let cost_to_current = get_fusion_cost_from_zero("Rare", 3, false);
        let cost_to_max = get_fusion_cost_from_zero("Rare", 10, false);
        let endo_to_max = cost_to_max.saturating_sub(cost_to_current);
        assert!(
            endo_to_max > 0,
            "a rank‑3‑of‑10 mod must have nonzero endo cost remaining"
        );
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

#[cfg(test)]
mod upgrade_suggestion_tests {
    use super::*;

    #[test]
    fn unranked_mod_with_profitable_max_price_is_suggested() {
        // Regression test: get_fusion_cost_from_zero(rank=0) is 0 by definition, which used to
        // be (wrongly) used as a gate for "has this mod been touched at all" — excluding every
        // unranked mod, the single most common case for a sellable duplicate.
        let result = upgrade_suggestion("Rare", 0, 10, false, 10.0, 80.0, 500);
        assert!(
            result.is_some(),
            "an unranked mod with a profitable max price must still be suggested"
        );
        let (delta, endo_to_max, _score) = result.unwrap();
        assert!((delta - 70.0).abs() < f64::EPSILON);
        assert!(endo_to_max > 0);
    }

    #[test]
    fn already_maxed_mod_is_not_suggested() {
        assert!(upgrade_suggestion("Rare", 10, 10, false, 10.0, 80.0, 500).is_none());
    }

    #[test]
    fn unprofitable_upgrade_is_not_suggested() {
        // max_price <= wa_price: no point spending endo to "upgrade" into a cheaper or
        // equal price.
        assert!(upgrade_suggestion("Rare", 0, 10, false, 50.0, 40.0, 500).is_none());
    }
}

#[cfg(test)]
mod set_aggregation_tests {
    use super::*;
    use crate::models::{WfcdItem, WfmEn, WfmI18n, WfmItem};
    use std::collections::HashMap;

    fn build_test_wfm_item(slug: &str, name: &str) -> WfmItem {
        WfmItem {
            id: slug.to_string(),
            slug: slug.to_string(),
            game_ref: None,
            tags: vec![],
            max_rank: None,
            i18n: WfmI18n {
                en: WfmEn {
                    name: name.to_string(),
                },
            },
            subtypes: vec![],
            set_root: true,
            bulk_tradable: false,
            max_amber_stars: None,
            max_cyan_stars: None,
        }
    }

    fn build_test_candidate(slug: &str, name: &str, game_ref: &str, qty: u32) -> MappedItem {
        MappedItem {
            id: slug.to_string(),
            slug: slug.to_string(),
            name: name.to_string(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: "Common".to_string(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: game_ref.to_string(),
            subtypes: vec![],
            owned_subtype: None,
            bulk_tradable: false,
        }
    }

    #[test]
    fn exactly_one_set() {
        // Build: Mag Prime requires BP, Chassis, Neuroptics, Systems (1 each)
        let build_name = "Mag Prime";
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();

        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(
            build_unique.clone(),
            WfcdItem {
                unique_name: build_unique.clone(),
                name: build_name.to_string(),
                level_stats: None,
                category: None,
                rarity: None,
                fusion_limit: None,
                components: None,
            },
        );

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            (
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(),
                1,
            ),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        // WFM by name: set item and component items
        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert(
            "mag prime set".to_string(),
            build_test_wfm_item("mag_prime_set", "Mag Prime Set"),
        );
        wfm_by_name.insert(
            "mag prime blueprint".to_string(),
            build_test_wfm_item("mag_prime_blueprint", "Mag Prime Blueprint"),
        );
        wfm_by_name.insert(
            "mag prime chassis".to_string(),
            build_test_wfm_item("mag_prime_chassis", "Mag Prime Chassis"),
        );
        wfm_by_name.insert(
            "mag prime neuroptics".to_string(),
            build_test_wfm_item("mag_prime_neuroptics", "Mag Prime Neuroptics"),
        );
        wfm_by_name.insert(
            "mag prime systems".to_string(),
            build_test_wfm_item("mag_prime_systems", "Mag Prime Systems"),
        );

        // Parent map: each component maps to the build
        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        // Candidates: one of each component
        let candidates = vec![
            build_test_candidate(
                "mag_prime_blueprint",
                "Mag Prime Blueprint",
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint",
                1,
            ),
            build_test_candidate(
                "mag_prime_chassis",
                "Mag Prime Chassis",
                "/Lotus/Types/Recipes/Components/MagPrimeChassis",
                1,
            ),
            build_test_candidate(
                "mag_prime_neuroptics",
                "Mag Prime Neuroptics",
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics",
                1,
            ),
            build_test_candidate(
                "mag_prime_systems",
                "Mag Prime Systems",
                "/Lotus/Types/Recipes/Components/MagPrimeSystems",
                1,
            ),
        ];

        // Prices: set price 100, each component 10
        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 10.0);
        prices.insert("mag_prime_systems".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Expect exactly one set item, no leftover components
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].slug, "mag_prime_set");
        assert_eq!(result[0].quantity, 1);
    }

    #[test]
    fn two_sets_with_leftovers() {
        // Same as above but with extra parts
        // Build: Mag Prime (1 each)
        // Candidates: 2 BP, 2 Chassis, 2 Neuroptics, 5 Systems
        // Expect 2 sets, leftover 3 Systems

        let build_name = "Mag Prime";
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();

        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(
            build_unique.clone(),
            WfcdItem {
                unique_name: build_unique.clone(),
                name: build_name.to_string(),
                level_stats: None,
                category: None,
                rarity: None,
                fusion_limit: None,
                components: None,
            },
        );

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            (
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(),
                1,
            ),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert(
            "mag prime set".to_string(),
            build_test_wfm_item("mag_prime_set", "Mag Prime Set"),
        );
        wfm_by_name.insert(
            "mag prime blueprint".to_string(),
            build_test_wfm_item("mag_prime_blueprint", "Mag Prime Blueprint"),
        );
        wfm_by_name.insert(
            "mag prime chassis".to_string(),
            build_test_wfm_item("mag_prime_chassis", "Mag Prime Chassis"),
        );
        wfm_by_name.insert(
            "mag prime neuroptics".to_string(),
            build_test_wfm_item("mag_prime_neuroptics", "Mag Prime Neuroptics"),
        );
        wfm_by_name.insert(
            "mag prime systems".to_string(),
            build_test_wfm_item("mag_prime_systems", "Mag Prime Systems"),
        );

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        let candidates = vec![
            build_test_candidate(
                "mag_prime_blueprint",
                "Mag Prime Blueprint",
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint",
                2,
            ),
            build_test_candidate(
                "mag_prime_chassis",
                "Mag Prime Chassis",
                "/Lotus/Types/Recipes/Components/MagPrimeChassis",
                2,
            ),
            build_test_candidate(
                "mag_prime_neuroptics",
                "Mag Prime Neuroptics",
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics",
                2,
            ),
            build_test_candidate(
                "mag_prime_systems",
                "Mag Prime Systems",
                "/Lotus/Types/Recipes/Components/MagPrimeSystems",
                5,
            ),
        ];

        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 10.0);
        prices.insert("mag_prime_systems".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Expect 2 sets + 1 leftover systems (qty 3)
        assert_eq!(result.len(), 2);
        let sets: Vec<_> = result
            .iter()
            .filter(|i| i.slug == "mag_prime_set")
            .collect();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].quantity, 2);
        let leftovers: Vec<_> = result
            .iter()
            .filter(|i| i.slug == "mag_prime_systems")
            .collect();
        assert_eq!(leftovers.len(), 1);
        assert_eq!(leftovers[0].quantity, 3);
    }

    #[test]
    fn not_enough_parts_no_set() {
        // Missing one component -> no set
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();
        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(
            build_unique.clone(),
            WfcdItem {
                unique_name: build_unique.clone(),
                name: "Mag Prime".to_string(),
                level_stats: None,
                category: None,
                rarity: None,
                fusion_limit: None,
                components: None,
            },
        );

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            (
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(),
                1,
            ),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert(
            "mag prime set".to_string(),
            build_test_wfm_item("mag_prime_set", "Mag Prime Set"),
        );

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        let candidates = vec![
            build_test_candidate(
                "mag_prime_blueprint",
                "Mag Prime Blueprint",
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint",
                1,
            ),
            build_test_candidate(
                "mag_prime_chassis",
                "Mag Prime Chassis",
                "/Lotus/Types/Recipes/Components/MagPrimeChassis",
                1,
            ),
            build_test_candidate(
                "mag_prime_neuroptics",
                "Mag Prime Neuroptics",
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics",
                1,
            ),
            // missing systems
        ];

        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // No set, all parts remain as individual candidates
        assert_eq!(result.len(), 3);
        assert!(!result.iter().any(|i| i.slug == "mag_prime_set"));
    }

    #[test]
    fn expensive_component_does_not_prevent_bundling() {
        // Regression test for the Corinth/Akvasto/Phantasma Prime bug: a component priced
        // well above the assembled set's price (e.g. the part sourced from the rarest-tier
        // relic) must NOT block set formation. Warframe's Prime part market routinely prices
        // one component above 50%, even above 100%, of the set's own price — that's normal
        // relic-scarcity pricing, not a signal the bundle is a bad trade. A guard that killed
        // the whole set over this used to silently drop sets (and their components) from the
        // run entirely — see session log: "Set 'Corinth Prime' skipped: component 'Corinth
        // Prime Barrel' priced 45.3p exceeds 50% of set price 88.2p".
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();
        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(
            build_unique.clone(),
            WfcdItem {
                unique_name: build_unique.clone(),
                name: "Mag Prime".to_string(),
                level_stats: None,
                category: None,
                rarity: None,
                fusion_limit: None,
                components: None,
            },
        );

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            (
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(),
                1,
            ),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert(
            "mag prime set".to_string(),
            build_test_wfm_item("mag_prime_set", "Mag Prime Set"),
        );

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        let candidates = vec![
            build_test_candidate(
                "mag_prime_blueprint",
                "Mag Prime Blueprint",
                "/Lotus/Types/Recipes/Components/MagPrimeBlueprint",
                1,
            ),
            build_test_candidate(
                "mag_prime_chassis",
                "Mag Prime Chassis",
                "/Lotus/Types/Recipes/Components/MagPrimeChassis",
                1,
            ),
            build_test_candidate(
                "mag_prime_neuroptics",
                "Mag Prime Neuroptics",
                "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics",
                1,
            ),
            build_test_candidate(
                "mag_prime_systems",
                "Mag Prime Systems",
                "/Lotus/Types/Recipes/Components/MagPrimeSystems",
                1,
            ),
        ];

        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 60.0); // 60% of the 100p set price
        prices.insert("mag_prime_systems".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Set forms despite one component being priced above 50% of the set's price.
        let sets: Vec<_> = result
            .iter()
            .filter(|i| i.slug == "mag_prime_set")
            .collect();
        assert_eq!(
            sets.len(),
            1,
            "a complete set must form even with a disproportionately priced component"
        );
        assert_eq!(sets[0].quantity, 1);
    }

    #[test]
    fn filter_candidates_does_not_starve_aggregation_of_plainly_named_parts() {
        // Regression test for the Lato Vandal bug: Barrel/Receiver components whose display
        // names contain none of "prime"/"set"/"blueprint" (true of most non-Prime weapon
        // parts) used to get dropped by filter_candidates() before aggregate_sets_with_prices
        // ever ran, so a genuinely complete set could never be detected — its Blueprint (the
        // one component whose name happens to say "blueprint") would sit as a 100%
        // "unconsumed" leftover forever, and the Barrel/Receiver vanished entirely, never
        // shown at all. This exercises the real pipeline: filter_candidates() -> the aggregator.
        let build_name = "Lato Vandal";
        let build_unique = "/Lotus/Weapons/Tenno/Pistol/LatoVandal".to_string();

        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(
            build_unique.clone(),
            WfcdItem {
                unique_name: build_unique.clone(),
                name: build_name.to_string(),
                level_stats: None,
                category: None,
                rarity: None,
                fusion_limit: None,
                components: None,
            },
        );

        let recipe = vec![
            (
                "/Lotus/Types/Recipes/Weapons/LatoVandalBlueprint".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalBarrel".to_string(),
                1,
            ),
            (
                "/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalReceiver".to_string(),
                1,
            ),
        ];
        let mut requirements = BuildRequirements::new();
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert(
            "lato vandal set".to_string(),
            build_test_wfm_item("lato_vandal_set", "Lato Vandal Set"),
        );

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        // Matches the real report exactly: Barrel x1, Receiver x3, Blueprint x4 — none of
        // "Barrel"/"Receiver" contain prime/set/blueprint in their names.
        let owned = vec![
            build_test_candidate(
                "lato_vandal_blueprint",
                "Lato Vandal Blueprint",
                "/Lotus/Types/Recipes/Weapons/LatoVandalBlueprint",
                4,
            ),
            build_test_candidate(
                "lato_vandal_barrel",
                "Lato Vandal Barrel",
                "/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalBarrel",
                1,
            ),
            build_test_candidate(
                "lato_vandal_receiver",
                "Lato Vandal Receiver",
                "/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalReceiver",
                3,
            ),
        ];

        // Run through filter_candidates() first, exactly like the real pipeline does.
        let filtered = filter_candidates(owned, &parent_map);

        let mut prices = HashMap::new();
        prices.insert("lato_vandal_set".to_string(), 30.0);
        prices.insert("lato_vandal_blueprint".to_string(), 9.0);
        prices.insert("lato_vandal_barrel".to_string(), 9.0);
        prices.insert("lato_vandal_receiver".to_string(), 9.0);

        let result = aggregate_sets_with_prices(
            filtered,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Exactly 1 complete set formed (Barrel is the bottleneck at qty 1), plus 3 leftover
        // Blueprints (4 owned - 1 consumed). Leftover Receivers (2 spare) are pruned by the
        // post-aggregation "worth reviewing" heuristic, same as before this fix — the point of
        // this test is that the Set itself now forms and the Blueprint leftover is correctly
        // reduced, not left at the full unconsumed count of 4.
        let sets: Vec<_> = result
            .iter()
            .filter(|i| i.slug == "lato_vandal_set")
            .collect();
        assert_eq!(sets.len(), 1, "a complete Lato Vandal Set must be detected");
        assert_eq!(sets[0].quantity, 1);

        let blueprint_leftover: Vec<_> = result
            .iter()
            .filter(|i| i.slug == "lato_vandal_blueprint")
            .collect();
        assert_eq!(blueprint_leftover.len(), 1);
        assert_eq!(
            blueprint_leftover[0].quantity, 3,
            "3 spare blueprints should remain after 1 is consumed into the set"
        );
    }
}

// ── Candidate pricing (relocated from cli::pricing — Architecture Evolution Plan Phase 1.5) ──
//
// LiveStatsSource, build_priced_candidates, and sort_candidates used to live in cli/pricing.rs
// even though none of them are presentation code — they're the actual "how much is this
// candidate worth, and in what order should it be evaluated" algorithm that
// services::ListingSyncService (and, before it, cli::sell::run_cli directly) depends on.
// Moved here so services no longer has to reach back into cli:: for them. print_upgrade_suggestions
// stayed behind in cli/pricing.rs — it prints a formatted table, so it *is* presentation.

pub(crate) struct LiveStatsSource;

impl StatsSource for LiveStatsSource {
    async fn fetch(&self, slug: &str) -> AppResult<WfmStatsResponse> {
        fetch_statistics(slug).await
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn build_priced_candidates<S: StatsSource>(
    candidates: Vec<MappedItem>,
    _endo_rate: f64, // unused here, needed for signature compatibility
    parent_map: &BuildParentMap,
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    stats_source: &S,
) -> (
    Vec<(MappedItem, f64, f64, u32, f64)>,
    Vec<(String, f64, u32, u32, f64)>,
) {
    // ---- 1. Collect all slugs we need ----
    let mut slugs_to_fetch = HashSet::new();

    // All candidate slugs (including components)
    for item in &candidates {
        slugs_to_fetch.insert(item.slug.clone());
    }

    // Also, for each build, add its set slug (if resolvable)
    for build_unique in requirements.keys() {
        if let Some(wfcd_item) = wfcd_by_ref.get(build_unique)
            && let Some(set_item) = resolve_set_item(&wfcd_item.name, wfm_by_name)
        {
            slugs_to_fetch.insert(set_item.slug);
        }
    }

    // ---- 2. Fetch stats for all slugs, store in a map ----
    let mut stats_map = HashMap::new();
    for slug in &slugs_to_fetch {
        match stats_source.fetch(slug).await {
            Ok(stats) => {
                stats_map.insert(slug.clone(), stats);
            }
            Err(e) => tseprintln!("Warning: failed to fetch stats for {slug}: {e}"),
        }
    }

    // ---- 3. Build price map (slug -> wa_price) ----
    let mut price_map = HashMap::new();
    for (slug, stats) in &stats_map {
        let (wa_price, _) = calculate_weighted_average(stats, None);
        price_map.insert(slug.clone(), wa_price);
    }

    // ---- 4. Aggregate sets ----
    let aggregated_items = aggregate_sets_with_prices(
        candidates,
        parent_map,
        requirements,
        wfcd_by_ref,
        wfm_by_name,
        &price_map,
    );

    // ---- 5. For each aggregated item, compute pricing details ----
    let mut priced = Vec::new();
    let mut upgrades = Vec::new();

    for item in aggregated_items {
        // Determine the target rank for price calculation (mods/arcanes only)
        let target_rank = if item.is_mod || item.is_arcane {
            item.rank
        } else {
            None
        };

        // Get stats for this item's slug
        let stats_opt = stats_map.get(&item.slug);
        let (wa_price, _total_vol) = if let Some(stats) = stats_opt {
            calculate_weighted_average(stats, target_rank)
        } else {
            (0.0, 0)
        };

        if wa_price <= 0.0 {
            // Skip items that have no market price
            continue;
        }

        // Recent volume (30 days) — computed once and reused for both the demand-floor check
        // below and the score/display value further down.
        let (vol_30d, _trading_days_30d) = if let Some(stats) = stats_opt {
            recent_volume(stats, target_rank, 30)
        } else {
            (0, 0)
        };

        // For mods, also check volume *at max rank*. An unranked (rank 0) mod is a perfectly
        // good upgrade candidate even if the unranked market itself is thin — what actually
        // matters for "is it worth leveling this up" is whether the *maxed* copy sells, since
        // that's the form you'd list it in after upgrading. Previously the demand floor below
        // used only `vol_30d` (volume at the item's *current* rank), which silently zeroed out
        // upgrade suggestions for every mod sitting at rank 0 with a quiet unranked market —
        // even wildly popular mods, since most owned drops are unranked and unranked copies
        // trade far less than maxed ones.
        let vol_30d_max = if item.is_mod
            && !item.is_arcane
            && let Some(max_rank) = item.max_rank
            && let Some(stats) = stats_opt
        {
            recent_volume(stats, Some(max_rank), 30).0
        } else {
            vol_30d
        };

        // ---- Demand floor for mods/arcanes ----
        // Use whichever rank (current or max) has better liquidity — only drop the item if
        // it's illiquid in both forms.
        if item.is_mod || item.is_arcane {
            let vol_per_day = f64::from(vol_30d.max(vol_30d_max)) / 30.0;
            if vol_per_day < MIN_DAILY_VOLUME {
                continue; // below demand floor, skip entirely
            }
        }

        // Saturation ratio
        let saturation = if let Some(stats) = stats_opt {
            calculate_saturation_ratio(stats, target_rank)
        } else {
            0.0
        };

        // Score: price * (1 + ln(volume)) – used for sorting later
        let score = wa_price * (1.0 + f64::from(vol_30d)).ln();

        // ---- Upgrade suggestions (only for mods) ----
        if item.is_mod
            && !item.is_arcane
            && let Some(max_rank) = item.max_rank
            && let Some(stats) = stats_opt
        {
            let current_rank_u32 = u32::from(item.rank.unwrap_or(0));
            let max_rank_u32 = u32::from(max_rank);
            let is_antique = is_antique(&item.slug, &item.game_ref);
            let (max_price, _) = calculate_weighted_average(stats, Some(max_rank));
            // Use vol_30d_max here (not vol_30d): the score/volume shown should reflect
            // demand for the mod in the form it'll actually be sold in after upgrading.
            if let Some((delta, endo_to_max, upgrade_score)) = upgrade_suggestion(
                &item.rarity,
                current_rank_u32,
                max_rank_u32,
                is_antique,
                wa_price,
                max_price,
                vol_30d_max,
            ) {
                upgrades.push((
                    item.name.clone(),
                    delta,
                    endo_to_max,
                    vol_30d_max,
                    upgrade_score,
                ));
            }
        }

        // Store priced candidate
        priced.push((item, wa_price, saturation, vol_30d, score));
    }

    // Sort upgrades by score descending and truncate
    let mut upgrades_sorted = upgrades;
    upgrades_sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    upgrades_sorted.truncate(15);

    (priced, upgrades_sorted)
}

#[cfg(test)]
mod build_priced_candidates_tests {
    use super::*;
    use crate::models::{WfmStatsItem, WfmStatsPayload, WfmStatsSubPayload};

    /// In-memory `StatsSource` for tests — maps slug -> pre-built `WfmStatsResponse`, so a test
    /// never touches the network or the on-disk stats cache that `LiveStatsSource` uses.
    struct FixtureStatsSource(HashMap<String, WfmStatsResponse>);

    impl StatsSource for FixtureStatsSource {
        async fn fetch(&self, slug: &str) -> AppResult<WfmStatsResponse> {
            self.0
                .get(slug)
                .cloned()
                .ok_or_else(|| format!("no fixture stats for slug '{slug}'").into())
        }
    }

    /// Builds one day's worth of stats at a given mod rank — enough for `recent_volume`,
    /// `calculate_weighted_average`, and `calculate_saturation_ratio` to all see a single,
    /// recent, unambiguous data point. `order_type: "sell"` matters here specifically because
    /// `calculate_saturation_ratio` filters the live sub-payload on `order_type == "sell"`, and
    /// this fixture reuses the same item list for both the closed and live sub-payloads.
    fn stats_item(mod_rank: u32, wa_price: f64, volume: u32) -> WfmStatsItem {
        WfmStatsItem {
            datetime: "2026-06-20T00:00:00.000Z".to_string(),
            volume,
            min_price: wa_price,
            max_price: wa_price,
            avg_price: Some(wa_price),
            wa_price,
            median: wa_price,
            moving_avg: None,
            rank: Some(mod_rank),
            order_type: Some("sell".to_string()),
        }
    }

    fn stats_response(items: Vec<WfmStatsItem>) -> WfmStatsResponse {
        WfmStatsResponse {
            payload: WfmStatsPayload {
                statistics_closed: WfmStatsSubPayload {
                    ninety_days: items.clone(),
                },
                statistics_live: WfmStatsSubPayload { ninety_days: items },
            },
        }
    }

    fn unranked_mod_candidate(slug: &str, max_rank: u8) -> MappedItem {
        MappedItem {
            id: format!("{slug}_id"),
            slug: slug.to_string(),
            name: "Test Mod".to_string(),
            quantity: 3,
            rank: Some(0),
            max_rank: Some(max_rank),
            rarity: "Rare".to_string(),
            is_mod: true,
            is_arcane: false,
            is_ayatan: false,
            game_ref: "/Lotus/Upgrades/Mods/TestMod".to_string(),
            subtypes: vec![],
            owned_subtype: None,
            bulk_tradable: false,
        }
    }

    #[tokio::test]
    async fn unranked_mod_surfaces_an_upgrade_suggestion_end_to_end() {
        // Regression test for the bug where get_fusion_cost_from_zero(rank=0) == 0 was used as
        // a gate, silently excluding every unranked mod — the most common real candidate — from
        // ever reaching the upgrade list. This exercises the full build_priced_candidates
        // pipeline (slug collection, stats fetch via the seam, demand floor, scoring, and the
        // upgrade-suggestion block) rather than just the pure upgrade_suggestion() function.
        let candidate = unranked_mod_candidate("primed_pressure_point", 10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "primed_pressure_point".to_string(),
            stats_response(vec![
                stats_item(0, 10.0, 500),  // current (unranked) price/volume
                stats_item(10, 80.0, 500), // max-rank price
            ]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (priced, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0, // endo_rate, unused by this path
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert_eq!(priced.len(), 1, "the candidate should still be priced");
        assert!(
            !upgrades.is_empty(),
            "an unranked mod with a profitable max price must surface an upgrade suggestion"
        );
        assert_eq!(upgrades[0].0, "Test Mod");
    }

    #[tokio::test]
    async fn already_maxed_mod_does_not_surface_an_upgrade_suggestion() {
        let mut candidate = unranked_mod_candidate("maxed_mod", 10);
        candidate.rank = Some(10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "maxed_mod".to_string(),
            stats_response(vec![stats_item(10, 80.0, 500)]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (_, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0,
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert!(
            upgrades.is_empty(),
            "an already-maxed mod has nothing left to upgrade into"
        );
    }

    #[tokio::test]
    async fn low_volume_mod_is_filtered_before_reaching_the_upgrade_check() {
        // Below config::MIN_DAILY_VOLUME — should be skipped by the demand floor before
        // the upgrade-suggestion block ever runs, even though it would otherwise qualify.
        let candidate = unranked_mod_candidate("illiquid_mod", 10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "illiquid_mod".to_string(),
            stats_response(vec![
                stats_item(0, 10.0, 1), // 1 sale in 30 days, well under the 9.0/day floor
                stats_item(10, 80.0, 1),
            ]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (priced, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0,
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert!(
            priced.is_empty(),
            "below the demand floor, the candidate should be dropped entirely"
        );
        assert!(upgrades.is_empty());
    }

    #[tokio::test]
    async fn quiet_unranked_but_liquid_maxed_mod_still_surfaces_an_upgrade() {
        // Regression test: most owned mod copies sit at rank 0 (fresh drops), and the
        // *unranked* market for a mod is routinely much quieter than its maxed market even
        // for mods that are extremely popular once leveled (e.g. Serration-tier mods — nobody
        // farms endo to buy an unranked one, everyone wants it maxed). Gating the demand floor
        // (and the upgrade score) on current-rank volume alone silently dropped exactly this
        // case. Volume at rank 0 is below the floor; volume at max rank is well above it — the
        // item must still be priced and must still surface an upgrade suggestion.
        let candidate = unranked_mod_candidate("popular_when_maxed", 10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "popular_when_maxed".to_string(),
            stats_response(vec![
                stats_item(0, 10.0, 2),    // unranked: 2 sales/30d, well under the floor
                stats_item(10, 80.0, 900), // maxed: 900 sales/30d, comfortably liquid
            ]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (priced, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0,
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert_eq!(
            priced.len(),
            1,
            "liquid-when-maxed candidate should still be priced"
        );
        assert!(
            !upgrades.is_empty(),
            "should still surface an upgrade suggestion despite thin unranked volume"
        );
        assert_eq!(upgrades[0].0, "Test Mod");
    }
}

pub(crate) fn sort_candidates(
    mut priced: Vec<(MappedItem, f64, f64, u32, f64)>,
    existing: &HashMap<ListingKey, Vec<OwnedOrder>>,
) -> Vec<(MappedItem, f64, f64, u32, f64)> {
    priced.sort_by(|a, b| {
        let a_key = ListingKey {
            item_id: a.0.id.clone(),
            rank: if a.0.is_mod || a.0.is_arcane {
                a.0.rank
            } else {
                None
            },
        };
        let b_key = ListingKey {
            item_id: b.0.id.clone(),
            rank: if b.0.is_mod || b.0.is_arcane {
                b.0.rank
            } else {
                None
            },
        };
        let a_listed = existing.contains_key(&a_key);
        let b_listed = existing.contains_key(&b_key);
        if a_listed && !b_listed {
            std::cmp::Ordering::Less
        } else if !a_listed && b_listed {
            std::cmp::Ordering::Greater
        } else {
            // .1 = wa_price (list price), not .4 (score) — descending
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    priced
}
