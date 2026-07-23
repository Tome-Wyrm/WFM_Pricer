//! Phase 2 — Repository Layer
//!
//! Repositories sit between application services and storage. Per the
//! architecture plan: "initially repositories may wrap existing JSON or
//! in-memory implementations... later they become SQLite-backed without
//! affecting domain logic." This module's initial implementations wrap
//! the JSON/TOML files this crate already reads and writes elsewhere:
//!
//! - `StatisticsRepositoryJson` → `cache/statistics/` (per-item files)
//! - `MarketRepositoryJson` → `cache/metadata_cache.json`
//! - `ReferenceRepositoryJson` → `cache/vendors_raw_cache.json` (via
//!   `vendor::raw`)
//! - `VendorRepositoryToml` → `config/vendors.toml` (via
//!   `vendor::metadata`)
//! - `InventoryRepositoryJson`, `SettingsRepositoryToml` → stubs; their
//!   real sources weren't part of this pass.
//!
//! Acceptance criteria this module works toward:
//! - Business logic does not execute SQL/file IO directly.
//! - Storage implementation is replaceable behind these traits.
//! - Validation occurs at repository boundaries.

mod json_backend;
mod traits;

pub use traits::{
    InventoryRepository, MarketRepository, ReferenceRepository, RepositoryError,
    SettingsRepository, StatisticsRepository, VendorRepository,
};

mod inventory_repository;
mod market_repository;
mod reference_repository;
mod settings_repository;
mod statistics_repository;
mod vendor_repository;

pub use inventory_repository::InventoryRepositoryJson;
pub use market_repository::{MarketRepositoryJson, REFRESH_HISTORY_KEY};
pub use reference_repository::ReferenceRepositoryJson;
pub use settings_repository::SettingsRepositoryToml;
pub use statistics_repository::StatisticsRepositoryJson;
pub use vendor_repository::VendorRepositoryToml;
