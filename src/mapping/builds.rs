//! Build-parent/component-requirements maps (derived from WFCD's `All.json`), resolving a
//! build's WFM "Set" item, tracking build status against mastery/ownership, and finding
//! partially-assembled Sets.

use crate::AppResult;
use std::collections::{HashMap, HashSet};

use crate::models::{MappedItem, WfcdItem, WfmItem};

use super::{find_wfm_match, load_lookup_tables};

/// Mapping from a component's `uniqueName` to its parent build's `uniqueName`.
///
/// `BTreeMap`, not `HashMap`: this map is *iterated* (not just looked up) by the greedy
/// set-formation pass in `cli::aggregation::aggregate_sets_with_prices` and by
/// `find_incomplete_sets` below, where the order builds are processed in determines which
/// build "wins" a contested shared component and the order results are reported in. A
/// `HashMap`'s iteration order is randomized per-process (`SipHash` + random seed), which made
/// both non-deterministic across runs on identical input. `BTreeMap` iterates in sorted-key
/// order, which is deterministic and stable for free — and both maps are only ever built
/// once per run from a fixed cache file and then read many times, so the O(log n) insert cost
/// relative to `HashMap` is negligible.
pub type BuildParentMap = std::collections::BTreeMap<String, String>;

/// Mapping from a build's `uniqueName` to its list of required components and quantities.
/// `BTreeMap` for the same reason as `BuildParentMap` above.
pub type BuildRequirements = std::collections::BTreeMap<String, Vec<(String, u32)>>;

/// Pure helper: builds the parent map and requirements map from an already-parsed list of WFCD
/// items. Split out from `load_build_maps` so the parsing logic can be unit tested against a
/// small fixture slice without touching the filesystem.
#[must_use]
pub fn build_maps_from_items(
    wfcd_items: Vec<WfcdItem>,
    wfm_by_ref: Option<&HashMap<String, WfmItem>>,
    wfm_by_name: Option<&HashMap<String, WfmItem>>,
    wfcd_by_ref: Option<&HashMap<String, WfcdItem>>,
) -> (BuildParentMap, BuildRequirements) {
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
            //
            // UPDATE: Check warframe.market items cache first if lookups are available,
            // as WFCD's `tradable` field can be incorrect or out of date.
            let tradable_components: Vec<_> = components
                .into_iter()
                .filter(|comp| {
                    if let (Some(wfm_ref), Some(wfm_name), Some(wfcd_ref)) =
                        (wfm_by_ref, wfm_by_name, wfcd_by_ref)
                    {
                        wfm_ref.contains_key(&comp.unique_name)
                            || wfcd_ref
                                .get(&comp.unique_name)
                                .and_then(|wfcd_item| find_wfm_match(&wfcd_item.name, wfm_name))
                                .is_some()
                    } else {
                        comp.tradable
                    }
                })
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
pub fn load_build_maps() -> AppResult<(BuildParentMap, BuildRequirements)> {
    let cache_path = crate::config::WFCD_CACHE_FILE;
    if !std::path::Path::new(cache_path).exists() {
        return Err("WFCD cache file missing. Run update_caches first.".into());
    }
    let raw = std::fs::read_to_string(cache_path)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&raw)?;

    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, _) = load_lookup_tables()?;

    Ok(build_maps_from_items(
        wfcd_items,
        Some(&wfm_by_ref),
        Some(&wfm_by_name),
        Some(&wfcd_by_ref),
    ))
}

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

/// One component still needed to complete a Set the user has partially assembled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingComponent {
    /// The component's WFCD `uniqueName` (matches keys in `BuildRequirements`/`BuildParentMap`).
    pub unique_name: String,
    /// Display name, resolved from the WFCD cache where possible.
    pub name: String,
    /// How many more copies are needed to reach the recipe's required quantity.
    pub deficit: u32,
}

/// A Set the user owns at least one component of, but not enough of every component to
/// assemble yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteSet {
    /// The build's `uniqueName` (key into `wfcd_by_ref` / `BuildRequirements`).
    pub build_unique: String,
    pub missing: Vec<MissingComponent>,
}

/// Finds every build where the user owns at least one required component but is short on at
/// least one other, i.e. a Set that's genuinely worth considering completing (as opposed to
/// one they own zero parts of, or one they can already assemble). Builds with no tradeable
/// WFM Set listing (cosmetics, companion parts, etc.) are excluded entirely — there's nothing
/// to price or complete-for-profit there. Pure and synchronous by design — pricing the
/// shortfall against live buy orders is a separate, async concern handled by the caller (see
/// `cli::run_check_sets_cli`).
#[must_use]
pub fn find_incomplete_sets(
    mapped_items: &[MappedItem],
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
) -> Vec<IncompleteSet> {
    let mut owned: HashMap<&str, u32> = HashMap::new();
    for item in mapped_items {
        *owned.entry(item.game_ref.as_str()).or_insert(0) += item.quantity;
    }

    let mut result = Vec::new();
    for (build_unique, recipe) in requirements {
        // Skip builds with no tradeable WFM Set listing at all (cosmetics, companion
        // parts, and other non-marketable builds) before doing any ownership work —
        // there's nothing to price or complete-for-profit here, and surfacing these
        // just buries the builds that actually matter under warnings.
        let Some(wfcd_item) = wfcd_by_ref.get(build_unique) else {
            continue;
        };
        if resolve_set_item(&wfcd_item.name, wfm_by_name).is_none() {
            continue;
        }

        let mut any_owned = false;
        let mut missing = Vec::new();

        for (comp_unique, required_qty) in recipe {
            let have = owned.get(comp_unique.as_str()).copied().unwrap_or(0);
            if have > 0 {
                any_owned = true;
            }
            if have < *required_qty {
                // Prefer the WFM display name (what a trader actually recognizes) over
                // WFCD's, and only fall back to the raw uniqueName path as a last resort.
                let name = wfm_by_ref
                    .get(comp_unique)
                    .map(|w| w.i18n.en.name.clone())
                    .or_else(|| wfcd_by_ref.get(comp_unique).map(|w| w.name.clone()))
                    .unwrap_or_else(|| comp_unique.clone());
                missing.push(MissingComponent {
                    unique_name: comp_unique.clone(),
                    name,
                    deficit: required_qty - have,
                });
            }
        }

        // Own at least one part, but not the whole recipe yet -- that's "incomplete", as
        // distinct from "not started" (own nothing) or "already complete" (missing is empty).
        if any_owned && !missing.is_empty() {
            result.push(IncompleteSet {
                build_unique: build_unique.clone(),
                missing,
            });
        }
    }

    result
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
            i18n: WfmI18n {
                en: WfmEn {
                    name: "Mag Prime Set".into(),
                },
            },
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
            i18n: WfmI18n {
                en: WfmEn {
                    name: "Mag Prime Chassis".into(),
                },
            },
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
        let wfcd_items: Vec<WfcdItem> =
            serde_json::from_str(fixture).expect("fixture should parse");
        let (parent_map, requirements_map) = build_maps_from_items(wfcd_items, None, None, None);

        let recipe = requirements_map
            .get("/Lotus/Powersuits/Mag/MagPrime")
            .expect("Mag Prime should have a recorded recipe");
        assert_eq!(recipe.len(), 4);
        assert!(recipe.contains(&(
            "/Lotus/Weapons/Tenno/Blueprints/MagPrimeBlueprint".to_string(),
            1
        )));
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
            get_build_status(
                "untracked_part",
                &parent_map,
                &HashSet::new(),
                &HashSet::new()
            ),
            BuildStatus::Unknown
        );
    }
}
