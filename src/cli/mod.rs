//! CLI entry points and shared types.

// ── Imports used by every submodule ───────────────────────────────────
use crate::mapping::{BuildParentMap, BuildRequirements, BuildStatus, get_build_status};
use crate::models::{BlacklistConfig, KeepConfig, MappedItem, WfcdItem, WfmItem};
use crate::pricing::{
    calculate_weighted_average, fetch_statistics, get_ayatan_endo_yield, recent_volume,
};
use crate::wfm_client::{CreateOrder, Order as OwnedOrder, UpdateOrder, WfmClient};
use crate::{tseprintln, tsprint, tsprintln};
use std::collections::{HashMap, HashSet};
use std::io;

// ── Submodule declarations ─────────────────────────────────────────────
mod candidate;
mod check_sets;
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
pub(crate) use crate::config_io::{add_to_keeplist, get_keep_quantity, save_blacklist};
pub(crate) use crate::wfm_client::ListingKey;
pub(crate) use candidate::*;
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
