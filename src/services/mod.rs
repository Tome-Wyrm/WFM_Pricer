//! Application service layer (Architecture Evolution Plan, Phase 1.5).
//!
//! Services coordinate multiple domain modules (`mapping`, `ingestion`, `pricing`,
//! `vendor`, `wfm_client`, ...) into a single presentation-independent workflow, so the
//! same code path can be called from the CLI today and a future GUI without either one
//! reimplementing the coordination. The rule that makes this hold:
//!
//! - Services may depend on domain modules.
//! - Domain modules must never depend on services.
//! - Services return structured data (plain structs), never formatted text — printing,
//!   prompting, and table-rendering stay in `cli/` (or a future `gui/`).
//!
//! This module is filled in incrementally: each service below started life duplicated
//! across two or more `cli/` command handlers, and was pulled out here once the
//! duplication became a real (not hypothetical) maintenance risk. See each service's
//! doc comment for exactly what it replaced.

mod inventory_import;
mod listing_sync;
mod set_analysis;
mod vendor_analysis;

pub use inventory_import::{ImportedInventory, InventoryImportService};
pub(crate) use listing_sync::ListingSyncService;
pub use set_analysis::{
    IncompleteSetInfo, PricedIncompleteSet, SetAnalysis, SetAnalysisService, SetPricingResult,
};
pub(crate) use vendor_analysis::VendorAnalysisService;
