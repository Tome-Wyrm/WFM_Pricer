// src/vendor/matching.rs
//! Combines raw vendors/offerings with the `vendors.toml` overlay and WFM slug
//! matching into the `MappedVendor`/`MappedOffering` cache (Phase D3/D4).
use super::metadata::{
    CostMode, is_tradeable_category, load_vendor_metadata, normalize_item_name, target_rank_for,
};
use super::raw::{CurrencySpec, PrereqSpec, PriceSpec, RawOffering, load_vendor_data};
use crate::AppResult;
use crate::config;
use crate::tsprintln;
use serde::{Deserialize, Serialize};
use std::fs;

/// A `RawOffering` combined with its `vendors.toml` overlay context and WFM match
/// result — the per-offering row of `cache/vendors_cache.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedOffering {
    pub name: String,
    pub category: String,
    pub price: PriceSpec,
    pub qty: u32,
    pub prereq: Option<PrereqSpec>,
    pub timer: Option<u64>,
    pub limit: Option<u32>,
    /// `None` (with `unmatched_reason` set) if the category isn't tradeable or no WFM
    /// item name matched.
    pub wfm_slug: Option<String>,
    pub target_rank: Option<u32>,
    pub unmatched_reason: Option<String>,
}

/// A `RawVendor` combined with its `vendors.toml` metadata overlay and mapped
/// offerings — the per-vendor row of `cache/vendors_cache.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedVendor {
    pub key: String,
    pub name: String,
    pub currency: CurrencySpec,
    pub location: Option<String>,
    pub group: Option<String>,
    pub excluded: bool,
    pub cost_mode: CostMode,
    pub hand_curated: bool,
    pub offerings: Vec<MappedOffering>,
}

/// Attempts to resolve a raw offering's WFM slug via the existing
/// `mapping::find_wfm_match` lookup, after stripping any yield multiplier (D2).
fn match_offering(
    offering: &RawOffering,
    wfm_by_name: &std::collections::HashMap<String, crate::models::WfmItem>,
) -> (Option<String>, Option<String>) {
    let normalized = normalize_item_name(&offering.name);
    match crate::mapping::find_wfm_match(&normalized, wfm_by_name) {
        Some(item) => (Some(item.slug.clone()), None),
        None => (None, Some(format!("no WFM match for '{normalized}'"))),
    }
}

/// D3: builds the processed vendor cache — raw offerings + `vendors.toml` overlay +
/// matched WFM slug (or `None` + reason if unmatched) — and writes it to
/// `cache/vendors_cache.json`.
///
/// # Errors
/// Returns an error if the raw vendor cache, `vendors.toml`, or the WFM lookup tables
/// can't be loaded, or if the resulting cache can't be serialized/written.
pub fn build_and_write_vendor_cache() -> AppResult<Vec<MappedVendor>> {
    let raw_vendors = load_vendor_data()?;
    let meta = load_vendor_metadata()?;
    let (_, _, wfm_by_name, _) = crate::mapping::load_lookup_tables()?;

    let mapped: Vec<MappedVendor> = raw_vendors
        .into_iter()
        .map(|v| {
            let m = meta.get(&v.key).cloned().unwrap_or_default();
            let offerings = v
                .offerings
                .into_iter()
                .map(|o| {
                    let (slug, reason) = if is_tradeable_category(&o.category) {
                        match_offering(&o, &wfm_by_name)
                    } else {
                        (
                            None,
                            Some(format!("category '{}' not tradeable", o.category)),
                        )
                    };
                    MappedOffering {
                        target_rank: target_rank_for(&o.category),
                        name: o.name,
                        category: o.category,
                        price: o.price,
                        qty: o.qty,
                        prereq: o.prereq,
                        timer: o.timer,
                        limit: o.limit,
                        wfm_slug: slug,
                        unmatched_reason: reason,
                    }
                })
                .collect();
            MappedVendor {
                key: v.key,
                name: v.name,
                currency: v.currency,
                location: m.location,
                group: m.group,
                excluded: m.excluded,
                cost_mode: m.cost_mode,
                hand_curated: m.hand_curated,
                offerings,
            }
        })
        .collect();

    let json = serde_json::to_string_pretty(&mapped)?;
    fs::write(config::VENDORS_CACHE_FILE, json)?;
    Ok(mapped)
}

// ---- D4: match-coverage report ----

/// Per-vendor match statistics for `vendor --match-report`.
#[derive(Debug, Clone)]
pub struct VendorMatchStats {
    pub key: String,
    pub total_offerings: usize,
    pub tradeable_count: usize,
    pub matched_count: usize,
    /// Names of offerings that were in a tradeable category but didn't resolve to a
    /// WFM slug.
    pub unmatched: Vec<String>,
}

#[must_use]
pub fn compute_match_stats(vendors: &[MappedVendor]) -> Vec<VendorMatchStats> {
    vendors
        .iter()
        .map(|v| {
            let total_offerings = v.offerings.len();
            let tradeable: Vec<&MappedOffering> = v
                .offerings
                .iter()
                .filter(|o| is_tradeable_category(&o.category))
                .collect();
            let matched_count = tradeable.iter().filter(|o| o.wfm_slug.is_some()).count();
            let unmatched = tradeable
                .iter()
                .filter(|o| o.wfm_slug.is_none())
                .map(|o| o.name.clone())
                .collect();
            VendorMatchStats {
                key: v.key.clone(),
                total_offerings,
                tradeable_count: tradeable.len(),
                matched_count,
                unmatched,
            }
        })
        .collect()
}

/// Prints the D4 match-coverage report: per vendor, total / tradeable / matched /
/// unmatched counts, plus the unmatched item names. Internally,
/// `matched + unmatched + skipped-by-category == total` always holds by construction
/// (asserted below in debug builds) since every offering falls into exactly one of
/// those three buckets.
pub fn print_match_report(vendors: &[MappedVendor]) {
    let stats = compute_match_stats(vendors);
    tsprintln!(
        "{:<28} {:>6} {:>10} {:>8} {:>10}",
        "Vendor",
        "Total",
        "Tradeable",
        "Matched",
        "Unmatched"
    );
    for s in &stats {
        let unmatched_count = s.unmatched.len();
        let skipped = s.total_offerings - s.tradeable_count;
        debug_assert_eq!(
            s.matched_count + unmatched_count + skipped,
            s.total_offerings
        );
        tsprintln!(
            "{:<28} {:>6} {:>10} {:>8} {:>10}",
            s.key,
            s.total_offerings,
            s.tradeable_count,
            s.matched_count,
            unmatched_count
        );
        for name in &s.unmatched {
            tsprintln!("    unmatched: {name}");
        }
    }
}

/// Ad hoc audit, grouped by category instead of by vendor: total offerings, matched
/// count, and up to 5 sample unmatched names per category. `is_tradeable_category`
/// only gates whether a lookup is attempted — this makes it visible whether a
/// category is a clean single-tradeability bucket or a genuine mix, instead of having
/// to infer that from the per-vendor D4 report.
pub fn dump_category_audit(vendors: &[MappedVendor]) {
    use std::collections::BTreeMap;
    let mut by_category: BTreeMap<&str, (usize, usize, Vec<&str>)> = BTreeMap::new();
    for v in vendors {
        for o in &v.offerings {
            let entry = by_category
                .entry(o.category.as_str())
                .or_insert((0, 0, Vec::new()));
            entry.0 += 1;
            if o.wfm_slug.is_some() {
                entry.1 += 1;
            } else if entry.2.len() < 5 {
                entry.2.push(o.name.as_str());
            }
        }
    }
    tsprintln!(
        "{:<20} {:>6} {:>8}  sample unmatched",
        "Category",
        "Total",
        "Matched"
    );
    for (cat, (total, matched, samples)) in by_category {
        tsprintln!("{cat:<20} {total:>6} {matched:>8}  {}", samples.join(", "));
    }
}

#[cfg(test)]
mod slug_matching_tests {
    use super::*;
    use crate::tseprintln;

    /// D3 spot-check: run the real caches through `build_and_write_vendor_cache` and
    /// confirm a handful of known-good (vendor, item) pairs resolve to a WFM slug.
    /// Skips (rather than fails) if the caches this depends on haven't been generated
    /// yet — same convention as the other cache-dependent integration tests in this
    /// file. Add to `KNOWN_GOOD` as you hand-verify more pairs against WFM.
    #[test]
    fn build_vendor_cache_resolves_known_items() {
        let raw_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        let wfm_path = std::path::Path::new(config::WFM_CACHE_FILE);
        let wfcd_path = std::path::Path::new(config::WFCD_CACHE_FILE);
        if !raw_path.exists() || !wfm_path.exists() || !wfcd_path.exists() {
            tseprintln!("Skipping spot-check – caches not present. Run update-caches first.");
            return;
        }
        let mapped = build_and_write_vendor_cache().expect("failed to build vendor cache");

        // (vendor_key, offering_name) — hand-verified as actually tradeable on WFM.
        // Replace the placeholders below with real pairs from your vendors.toml/wiki
        // dump; PLACEHOLDER entries are skipped with a warning instead of failing, so
        // this test stays green while you fill the list in incrementally.
        const KNOWN_GOOD: &[(&str, &str)] = &[
            ("Acrithis", "Longbow Sharpshot"),
            ("Eleanor", "Primary Crux"),
            ("Arbiters of Hexis", "Decurion Barrel"),
            ("Steel Meridian", "Grineer Asteroid Simulacrum"),
            ("Palladino", "Requiem I Relic"),
        ];

        for (vendor_key, item_name) in KNOWN_GOOD {
            if *vendor_key == "PLACEHOLDER" {
                tseprintln!(
                    "Skipping unfilled KNOWN_GOOD placeholder — fill in real (vendor, item) pairs."
                );
                continue;
            }
            let vendor = mapped
                .iter()
                .find(|v| v.key == *vendor_key)
                .unwrap_or_else(|| panic!("vendor '{vendor_key}' missing from mapped cache"));
            let offering = vendor
                .offerings
                .iter()
                .find(|o| o.name == *item_name)
                .unwrap_or_else(|| {
                    panic!("offering '{item_name}' missing from vendor '{vendor_key}'")
                });
            assert!(
                offering.wfm_slug.is_some(),
                "'{item_name}' ({vendor_key}) should resolve to a WFM slug, got: {:?}",
                offering.unmatched_reason
            );
        }
    }
    #[test]
    #[ignore] // manual audit, not a pass/fail check — run with `cargo test -- --ignored dump_category_audit -- --nocapture`
    fn dump_category_audit_manual() {
        let raw_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        let wfm_path = std::path::Path::new(config::WFM_CACHE_FILE);
        let wfcd_path = std::path::Path::new(config::WFCD_CACHE_FILE);
        if !raw_path.exists() || !wfm_path.exists() || !wfcd_path.exists() {
            tseprintln!("Skipping category audit – caches not present. Run update-caches first.");
            return;
        }
        let mapped = build_and_write_vendor_cache().expect("failed to build vendor cache");
        dump_category_audit(&mapped);
    }
}

#[cfg(test)]

mod match_report_tests {
    use super::*;

    fn offering(name: &str, category: &str, slug: Option<&str>) -> MappedOffering {
        MappedOffering {
            name: name.to_string(),
            category: category.to_string(),
            price: PriceSpec::Single("Credits".to_string(), 100.0),
            qty: 1,
            prereq: None,
            timer: None,
            limit: None,
            wfm_slug: slug.map(str::to_string),
            target_rank: target_rank_for(category),
            unmatched_reason: if slug.is_some() {
                None
            } else {
                Some("test".to_string())
            },
        }
    }

    #[test]
    fn compute_match_stats_totals_reconcile() {
        let vendors = vec![MappedVendor {
            key: "Test".to_string(),
            name: "Test".to_string(),
            currency: CurrencySpec::One("Credits".to_string()),
            location: None,
            group: None,
            excluded: false,
            cost_mode: CostMode::Single,
            hand_curated: false,
            offerings: vec![
                offering("A", "Mod", Some("a_slug")),
                offering("B", "Mod", None),
                offering("C", "Sigil", None),
            ],
        }];

        let stats = compute_match_stats(&vendors);
        assert_eq!(stats.len(), 1);
        let s = &stats[0];
        assert_eq!(s.total_offerings, 3);
        assert_eq!(s.tradeable_count, 2); // Mod, Mod (Sigil is skipped-by-category)
        assert_eq!(s.matched_count, 1);
        assert_eq!(s.unmatched, vec!["B".to_string()]);

        // Matched + unmatched + skipped-by-category == total.
        let skipped = s.total_offerings - s.tradeable_count;
        assert_eq!(
            s.matched_count + s.unmatched.len() + skipped,
            s.total_offerings
        );
    }
}
