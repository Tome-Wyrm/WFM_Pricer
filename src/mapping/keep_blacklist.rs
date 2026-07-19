//! Keeplist/blacklist resolution: filtering blacklisted slugs, reserving kept quantities, and
//! merging/pooling duplicate mod & arcane entries across rank buckets before reservation runs.

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use crate::models::{BlacklistConfig, KeepConfig, MappedItem};

pub(crate) fn load_keep_blacklist() -> Result<(KeepConfig, BlacklistConfig), Box<dyn Error>> {
    let keep_map = if Path::new(crate::config::KEEPLIST_FILE).exists() {
        let raw = fs::read_to_string(crate::config::KEEPLIST_FILE)?;
        toml::from_str(&raw).map_err(|e| format!("Failed to parse keeplist.toml: {e:?}"))?
    } else {
        KeepConfig {
            defaults: HashMap::new(),
            items: HashMap::new(),
        }
    };

    let blacklist = if Path::new(crate::config::BLACKLIST_FILE).exists() {
        let raw = fs::read_to_string(crate::config::BLACKLIST_FILE)?;
        toml::from_str(&raw).map_err(|e| format!("Failed to parse blacklist.toml: {e:?}"))?
    } else {
        BlacklistConfig::default()
    };

    Ok((keep_map, blacklist))
}

/// Total raw-copy cost (including the base copy) to fuse an arcane to `rank` via
/// duplicate-consumption — triangular numbers: rank 5 needs 1+2+3+4+5+6 = 21 total copies.
pub(crate) fn arcane_rank_cost(rank: u8) -> u32 {
    match rank {
        1 => 3,
        2 => 6,
        3 => 10,
        4 => 15,
        5 => 21,
        _ => 1,
    }
}

pub(crate) fn apply_keep_blacklist(
    mut item: MappedItem,
    keep_map: &KeepConfig,
    blacklist: &BlacklistConfig,
) -> Option<MappedItem> {
    if blacklist.slugs.contains(&item.slug) {
        return None;
    }
    // Mods/arcanes are no longer reserved here: a raw inventory entry is a single duplicate
    // (quantity 1 in the common case), so comparing it against `keep` per-entry silently drops
    // every duplicate independently instead of reserving one copy across the total. Keep
    // resolution for these categories now happens once, after `merge_duplicate_ranked_items`
    // pools same-slug-same-rank entries — see `apply_cross_rank_keep` below.
    if item.is_mod || item.is_arcane {
        return Some(item);
    }
    let keep_reserved = {
        let rules = keep_map.items.get(&item.slug);
        if let Some(rules) = rules {
            let rank_val = item.rank;
            if let Some(rank) = rank_val {
                rules
                    .iter()
                    .find(|r| r.rank == Some(rank))
                    .or_else(|| rules.iter().find(|r| r.rank.is_none()))
                    .map_or(0, |r| r.keep)
            } else {
                rules
                    .iter()
                    .find(|r| r.rank.is_none())
                    .map_or(0, |r| r.keep)
            }
        } else {
            0
        }
    };
    if keep_reserved > 0 {
        if item.quantity <= keep_reserved {
            return None;
        }
        item.quantity -= keep_reserved;
    }
    Some(item)
}

/// Merges `MappedItem` entries that are the same underlying item at the same rank (mods and
/// arcanes only) into one entry with a summed quantity. Without this, every leveled duplicate
/// arrives as its own `quantity: 1` entry (see `map_single`/`process_item` — the `Upgrades`
/// inventory category has no `ItemCount`, one entry per copy), and keep-reservation compared
/// against each independently instead of the true total.
pub(crate) fn merge_duplicate_ranked_items(items: Vec<MappedItem>) -> Vec<MappedItem> {
    let mut merged: Vec<MappedItem> = Vec::new();
    let mut index: std::collections::HashMap<(String, Option<u8>), usize> =
        std::collections::HashMap::new();

    for item in items {
        if item.is_mod || item.is_arcane {
            let key = (item.id.clone(), item.rank);
            if let Some(&i) = index.get(&key) {
                merged[i].quantity += item.quantity;
                continue;
            }
            index.insert(key, merged.len());
        }
        merged.push(item);
    }
    merged
}

/// Reserve `keep_total` units, drawn from the highest-rank bucket first (`variants` must
/// already be sorted rank-descending), spilling into lower ranks only if the top bucket alone
/// doesn't have enough. With `keep_total == 1` this is exactly "keep whichever copy is
/// furthest along" — a maxed copy protects itself with nothing left over; with no maxed copy,
/// the best rank-0 duplicate is held back instead.
fn apply_simple_total_reserve(variants: &mut [MappedItem], keep_total: u32) {
    let mut remaining = keep_total;
    for item in variants.iter_mut() {
        let reserve = remaining.min(item.quantity);
        item.quantity -= reserve;
        remaining -= reserve;
    }
}

/// Mods only. `variants` must already be sorted rank-descending.
///
/// If `keeplist.toml` has any rank-specific rules for this slug, each reserves exactly that
/// many units at exactly that rank, and the pooled default is skipped entirely — explicit
/// per-rank rules mean "these specific ranks are spoken for," not "add to the pool." Otherwise
/// falls back to a rank-less item override, or the `defaults.mod` category default, pooled
/// across all ranks.
fn apply_mod_keep(
    variants: &mut [MappedItem],
    item_rules: Option<&Vec<crate::models::KeepRule>>,
    default_keep: u32,
) {
    if let Some(rules) = item_rules {
        let rank_specific: Vec<&crate::models::KeepRule> =
            rules.iter().filter(|r| r.rank.is_some()).collect();
        if !rank_specific.is_empty() {
            for rule in rank_specific {
                if let Some(item) = variants.iter_mut().find(|v| v.rank == rule.rank) {
                    let reserve = rule.keep.min(item.quantity);
                    item.quantity -= reserve;
                }
            }
            return;
        }
        if let Some(rankless) = rules.iter().find(|r| r.rank.is_none()) {
            apply_simple_total_reserve(variants, rankless.keep);
            return;
        }
    }
    apply_simple_total_reserve(variants, default_keep);
}

/// Arcanes only. `variants` must already be sorted rank-descending. Reserves 1 unit of the
/// highest-ranked copy owned (the one being kept/completed), plus however many rank-0 dupes
/// are still needed to fuse it up to `max_rank`. Everything beyond that — extra maxed copies,
/// extra raw dupes — is left sellable.
fn apply_arcane_fusion_reserve(variants: &mut [MappedItem], max_rank: Option<u8>) {
    let Some(max_rank) = max_rank else { return };
    let Some(base_idx) = variants.iter().position(|v| v.quantity > 0) else {
        return;
    };
    let base_rank = variants[base_idx].rank.unwrap_or(0);

    variants[base_idx].quantity -= 1;

    if base_rank >= max_rank {
        return;
    }
    let needed_raw = arcane_rank_cost(max_rank).saturating_sub(arcane_rank_cost(base_rank));
    if let Some(raw) = variants.iter_mut().find(|v| v.rank == Some(0)) {
        let reserve = needed_raw.min(raw.quantity);
        raw.quantity -= reserve;
    }
}

/// Applies keep-reservation once per underlying item, across all its rank buckets together,
/// for mods and arcanes. Must run after `merge_duplicate_ranked_items`. Non-mod/arcane items
/// pass through untouched — their keep-reservation already happened in `apply_keep_blacklist`.
pub(crate) fn apply_cross_rank_keep(
    items: Vec<MappedItem>,
    keep_map: &KeepConfig,
) -> Vec<MappedItem> {
    let mut by_item: std::collections::HashMap<String, Vec<MappedItem>> =
        std::collections::HashMap::new();
    let mut other = Vec::new();

    for item in items {
        if item.is_mod || item.is_arcane {
            by_item.entry(item.id.clone()).or_default().push(item);
        } else {
            other.push(item);
        }
    }

    let mut result = other;
    for (_, mut variants) in by_item {
        variants.sort_by(|a, b| b.rank.cmp(&a.rank));
        let sample_slug = variants[0].slug.clone();
        let sample_max_rank = variants[0].max_rank;
        let sample_is_arcane = variants[0].is_arcane;
        let item_rules = keep_map.items.get(&sample_slug);

        if sample_is_arcane {
            let keep_total = item_rules
                .and_then(|rules| rules.iter().find(|r| r.rank.is_none()))
                .map_or_else(
                    || keep_map.defaults.get("arcane").map_or(0, |r| r.keep),
                    |r| r.keep,
                );
            if keep_total > 0 {
                apply_arcane_fusion_reserve(&mut variants, sample_max_rank);
            }
        } else {
            let default_keep = keep_map.defaults.get("mod").map_or(0, |r| r.keep);
            apply_mod_keep(&mut variants, item_rules, default_keep);
        }

        result.extend(variants.into_iter().filter(|v| v.quantity > 0));
    }
    result
}
