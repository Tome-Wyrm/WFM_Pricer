use std::collections::HashMap;
use std::error::Error;

use super::{
    BuildParentMap, BuildRequirements, MappedItem, WfcdItem, WfmItem, WfmStatsResponse,
    get_fusion_cost_from_zero, resolve_set_item, tsprintln,
};

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
                let avail = *qty - consumed.get(comp_unique).copied().unwrap_or(0);
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

    for (comp_unique, (total_qty, comp_item_template)) in component_qty {
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

/// Abstraction over how `build_priced_candidates` retrieves market statistics for a slug.
/// Exists purely so tests can supply fixture data instead of making a real network call —
/// production code always uses `LiveStatsSource`, which just delegates to `fetch_statistics`.
pub(crate) trait StatsSource {
    async fn fetch(&self, slug: &str) -> Result<WfmStatsResponse, Box<dyn Error>>;
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
