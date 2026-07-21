//! CLI entry points and shared types.

// ── Imports used by every submodule ───────────────────────────────────
use crate::config::{BLACKLIST_FILE, KEEPLIST_FILE, MIN_DAILY_VOLUME};
use crate::mapping::{
    self, BuildParentMap, BuildRequirements, BuildStatus, get_build_status, resolve_set_item,
};
use crate::models::{
    BlacklistConfig, KeepConfig, KeepRule, MappedItem, WfcdItem, WfmItem, WfmStatsResponse,
};
use crate::pricing::{
    StatsSource, aggregate_sets_with_prices, calculate_saturation_ratio,
    calculate_weighted_average, derive_endo_to_plat_from_mods, fetch_statistics, filter_candidates,
    get_ayatan_endo_yield, is_antique, recent_volume,
    upgrade_suggestion,
};
use crate::wfm_client::{
    self, CreateOrder, Credentials, Order as OwnedOrder, UpdateOrder, WfmClient,
};
use crate::{tseprintln, tsprint, tsprintln};
use std::collections::{HashMap, HashSet};
use std::io;

// ── Submodule declarations ─────────────────────────────────────────────
mod candidate;
mod check_sets;
mod config_io;
mod data;
mod helpers;
mod pricing;
mod primed_mods;
mod report;
mod sell;
mod sell_relics;

// ── Re‑export public entry points ─────────────────────────────────────
pub use check_sets::run_check_sets_cli;
pub use primed_mods::run_primed_mod_prices;
pub use sell::run_cli;
pub use sell_relics::run_sell_relics_cli;

// ── Re‑export everything needed by submodules (via super::*) ──────────
pub(crate) use candidate::*;
pub(crate) use config_io::*;
pub(crate) use data::*;
pub(crate) use helpers::*;
pub(crate) use pricing::*;
pub(crate) use report::*;

// ── Shared types and constants ────────────────────────────────────────
pub(crate) const PRICE_TOLERANCE_PCT: f64 = 0.03;

/// WFM's maximum number of simultaneous sell listings per account.
pub(crate) const MAX_LISTING_SLOTS: usize = 100;

/// Minimum ratio of (market price / Endo-melt value) required before a sculpture is worth
/// listing rather than melting. Melting is instant and riskless; a market listing carries
/// time and platform-fee risk, so it needs a margin over the guaranteed Endo value.
pub(crate) const ENDO_LISTING_MARGIN: f64 = 1.15;

pub(crate) enum NoOpDecision {
    TrueNoOp,
    QuantitySyncOnly { new_quantity: u32, keep_price: u32 },
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ListingKey {
    pub(crate) item_id: String,
    pub(crate) rank: Option<u8>,
}

pub(crate) struct CandidateContext<'a> {
    pub(crate) existing_listings_map: &'a HashMap<ListingKey, Vec<OwnedOrder>>,
    pub(crate) wfm_client: &'a WfmClient,
    pub(crate) endo_rate: f64,
    pub(crate) blacklist_set: &'a mut BlacklistConfig,
    pub(crate) keeplist: &'a mut KeepConfig,
    pub(crate) active_slots_count: &'a mut usize,
    pub(crate) stdout: &'a mut io::Stdout,
    pub(crate) parent_map: &'a BuildParentMap,
    pub(crate) mastered_set: &'a HashSet<String>,
    pub(crate) owned_built_set: &'a HashSet<String>,
}
