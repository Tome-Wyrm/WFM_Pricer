//! `InventoryImportService` — resolves, ingests, and maps the player's inventory export,
//! plus the build/mastery/lookup context every downstream workflow needs alongside it.
//!
//! This is the "resolve path → ingest → map → load build maps → load lookup tables →
//! load mastery/ownership" sequence that used to be duplicated near-verbatim between
//! `app::run_default_pipeline` and `cli::check_sets::run_check_sets_cli` (and partially
//! again in `cli::sell_relics`). Pulling it into one service means the interactive CLI,
//! `--check-sets`, and a future GUI all call the same code path instead of each
//! re-deriving it and risking the two drifting out of sync.
//!
//! Deliberately does *not* call `mapping::update_caches` itself: whether to refresh
//! caches before importing is a per-workflow decision (the default pipeline and
//! `--check-sets` both want it; a hypothetical "re-run against what's already cached"
//! workflow might not), so callers that want a fresh cache call `mapping::update_caches`
//! first, same as before.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::mapping::{BuildParentMap, BuildRequirements};
use crate::models::{MappedItem, WfcdItem, WfmItem};
use crate::{AppResult, http, ingestion, mapping};

/// Everything a downstream workflow (interactive CLI, `--check-sets`, `--sell-relics`, a
/// future GUI, ...) needs after inventory import: the raw inventory JSON, the mapped
/// tradeable items, the build/mastery context, and the WFCD/WFM lookup tables used to
/// resolve names and requirements.
pub struct ImportedInventory {
    pub inventory: serde_json::Value,
    pub mapped: Vec<MappedItem>,
    pub parent_map: BuildParentMap,
    pub requirements: BuildRequirements,
    pub wfcd_by_ref: HashMap<String, WfcdItem>,
    pub wfm_by_ref: HashMap<String, WfmItem>,
    pub wfm_by_name: HashMap<String, WfmItem>,
    pub wfm_by_slug: HashMap<String, WfmItem>,
    pub mastered_set: HashSet<String>,
    pub owned_built_set: HashSet<String>,
    pub frame_tier_uniques: HashSet<String>,
}

/// Stateless application service: resolves the inventory file, ingests it, maps it to
/// tradeable WFM items, and loads the build/lookup/mastery context alongside it.
///
/// Prints nothing and makes no presentation decisions — it only returns structured
/// data. Callers decide what (if anything) to show the user.
pub struct InventoryImportService;

impl InventoryImportService {
    /// # Errors
    /// Returns an error if the inventory path can't be resolved, ingestion or mapping
    /// fails, or any of the on-disk build/lookup caches are missing or malformed.
    pub async fn import(inventory_override: Option<PathBuf>) -> AppResult<ImportedInventory> {
        let inventory_path = crate::app::resolve_inventory_path(inventory_override)?;
        let inventory = ingestion::ingest_inventory(&inventory_path)?;

        let mapped = mapping::map_inventory(&inventory, http::shared_client()).await?;

        let (parent_map, requirements) = mapping::load_build_maps()?;
        let (wfcd_by_ref, wfm_by_ref, wfm_by_name, wfm_by_slug) = mapping::load_lookup_tables()?;
        let (mastered_set, owned_built_set, frame_tier_uniques) =
            mapping::load_mastery_and_ownership(&inventory, &wfcd_by_ref);

        Ok(ImportedInventory {
            inventory,
            mapped,
            parent_map,
            requirements,
            wfcd_by_ref,
            wfm_by_ref,
            wfm_by_name,
            wfm_by_slug,
            mastered_set,
            owned_built_set,
            frame_tier_uniques,
        })
    }
}
