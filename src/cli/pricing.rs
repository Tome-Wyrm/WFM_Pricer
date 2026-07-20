use crate::mapping::{BuildParentMap, BuildRequirements};
use std::collections::{HashMap, HashSet};
use crate::AppResult;

use super::{
    ListingKey, MIN_DAILY_VOLUME, MappedItem, OwnedOrder, StatsSource, WfcdItem, WfmItem,
    WfmStatsResponse, aggregate_sets_with_prices, calculate_saturation_ratio,
    calculate_weighted_average, fetch_statistics, is_antique, print_header, recent_volume,
    resolve_set_item, tseprintln, tsprintln, upgrade_suggestion,
};

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

pub(crate) fn print_upgrade_suggestions(suggestions: &[(String, f64, u32, u32, f64)]) {
    let mut sorted = suggestions.to_vec();
    sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(15);

    print_header("Mod Upgrade Suggestions (Best Endo Value × Volume)");
    tsprintln!(
        "\x1B[1m  {:<35} | {:<14} | {:<12} | {:<10} | Score\x1B[0m",
        "Mod",
        "Δ Plat (→max)",
        "Endo Cost",
        "30d Vol"
    );
    tsprintln!("  {}", "-".repeat(82));
    for (name, delta, endo, vol, score) in &sorted {
        tsprintln!("  {name:<35} | {delta:<14.1} | {endo:<12} | {vol:<10} | {score:.4}");
    }
    tsprintln!();
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
