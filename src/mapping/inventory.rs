//! Top-level entry point: maps a full `AlecaFrame` inventory export into the list of tradeable
//! `MappedItem`s, wiring together lookup tables, item-type filtering, live subtype fetches, and
//! keep/blacklist reservation.

use std::error::Error;
use std::path::Path;

use crate::config::{WFCD_CACHE_FILE, WFM_CACHE_FILE};
use crate::models::MappedItem;
use crate::{tseprintln, tsprintln};

use super::{
    apply_cross_rank_keep, apply_keep_blacklist, fetch_full_item, load_full_items_cache,
    load_keep_blacklist, load_lookup_tables, load_relic_map, merge_duplicate_ranked_items,
    process_item, save_full_items_cache,
};

// ── Main mapping function ─────────────────────────────────────────────────────

/// Maps the `AlecaFrame` inventory JSON to a list of tradeable WFM items.
///
/// # Errors
/// Returns an error if:
/// - Cache files are missing.
/// - File I/O or JSON parsing fails.
/// - TOML parsing of keeplist or blacklist fails.
pub async fn map_inventory(
    inventory: &serde_json::Value,
    client: &reqwest::Client,
) -> Result<Vec<MappedItem>, Box<dyn Error>> {
    if !Path::new(WFCD_CACHE_FILE).exists() || !Path::new(WFM_CACHE_FILE).exists() {
        return Err("Cache files missing. Please run update_caches first.".into());
    }

    tsprintln!("Loading caches from disk for mapping...");
    let mut full_cache = load_full_items_cache()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, wfm_by_slug) = load_lookup_tables()?;
    let (keep_map, blacklist) = load_keep_blacklist()?;
    let relic_map = load_relic_map();

    let mut results = Vec::new();
    let allowed_keys = [
        "FlavourItems",
        "RawUpgrades",
        "Upgrades",
        "FusionTreasures",
        "Recipes",
        "MiscItems",
    ];

    // Total inventory entries across the allowed categories, used purely to render
    // "(N/total)" progress below — this does not change what gets fetched or how.
    let total_items: usize = inventory.as_object().map_or(0, |obj| {
        allowed_keys
            .iter()
            .filter_map(|&key| obj.get(key).and_then(|v| v.as_array()))
            .map(Vec::len)
            .sum()
    });
    let mut processed = 0usize;

    if let Some(obj) = inventory.as_object() {
        for &category_key in &allowed_keys {
            if let Some(arr) = obj.get(category_key).and_then(serde_json::Value::as_array) {
                for element in arr {
                    processed += 1;
                    if processed.is_multiple_of(50) {
                        tsprintln!("Fetching item details... ({processed}/{total_items})");
                    }
                    if let Some(mut mapped) = process_item(
                        element,
                        category_key,
                        &wfm_by_ref,
                        &wfm_by_name,
                        &wfcd_by_ref,
                        &wfm_by_slug,
                        &relic_map,
                    ) {
                        // Fetch full item for this slug to get subtypes
                        match fetch_full_item(&mapped.slug, client, &mut full_cache).await {
                            Ok(full) => {
                                mapped.subtypes = full.subtypes;
                                // The lookup tables built at startup can be stale relative to
                                // the live per-item endpoint, and `bulkTradable` determines
                                // whether WFM requires `perTrade` on order creation — trust the
                                // freshly-fetched value over whatever map_single/process_item
                                // guessed from the cached tables.
                                mapped.bulk_tradable = full.bulk_tradable;
                            }
                            Err(e) => {
                                tseprintln!(
                                    "Warning: Could not fetch full item for {}: {}",
                                    mapped.slug,
                                    e
                                );
                                mapped.subtypes = Vec::new();
                            }
                        }

                        // Apply keeplist / blacklist
                        if let Some(final_item) =
                            apply_keep_blacklist(mapped, &keep_map, &blacklist)
                        {
                            results.push(final_item);
                        }
                    }
                }
            }
        }
    }

    // Save full items cache
    save_full_items_cache(&full_cache)?;

    let results = merge_duplicate_ranked_items(results);
    let results = apply_cross_rank_keep(results, &keep_map);
    Ok(results)
}
