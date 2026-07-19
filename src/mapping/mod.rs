//! Inventory mapping: turning a raw `AlecaFrame` inventory export into a list of tradeable WFM
//! items, plus the supporting build-recipe, mastery, and cache machinery.

// ── Submodule declarations ─────────────────────────────────────────────
mod ayatans;
mod builds;
mod cache;
mod filters;
mod inventory;
mod item_mapping;
mod keep_blacklist;
mod lookup;
mod mastery;
mod relics;

// ── Re‑export the public entry points used across the crate ───────────
pub use ayatans::{AMBER_STAR_REF, AYATANS, AyatanStaticDef, CYAN_STAR_REF};
pub use builds::{
    BuildParentMap, BuildRequirements, BuildStatus, IncompleteSet, MissingComponent,
    build_maps_from_items, find_incomplete_sets, get_build_status, load_build_maps,
    resolve_set_item,
};
pub use cache::{CacheMetadata, update_caches};
pub use inventory::map_inventory;
pub use mastery::{
    MASTERY_THRESHOLD_FRAME, MASTERY_THRESHOLD_NECRAMECH, MASTERY_THRESHOLD_OVERLEVEL_WEAPON,
    MASTERY_THRESHOLD_WEAPON, is_overlevel_gear, load_mastery_and_ownership, mastery_threshold,
};

// ── Re‑export everything needed by submodules (via super::*) ──────────
pub(crate) use cache::{fetch_full_item, load_full_items_cache, save_full_items_cache};
pub(crate) use filters::{check_allowlist, is_relic};
pub(crate) use item_mapping::process_item;
pub(crate) use keep_blacklist::{
    apply_cross_rank_keep, apply_keep_blacklist, load_keep_blacklist, merge_duplicate_ranked_items,
};
pub(crate) use lookup::{find_wfm_match, load_lookup_tables};
pub(crate) use relics::{load_relic_map, map_relic};
