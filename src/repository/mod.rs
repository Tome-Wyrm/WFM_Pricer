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
//! Phase 3 (SQLite persistence) migrates these one at a time onto the
//! three databases from the architecture plan, without changing the
//! traits above:
//!
//! - `StatisticsRepositorySqlite` → `market.db`
//! - `ReferenceRepositorySqlite` → `reference.db` (`vendors_raw` table)
//! - `VendorRepositorySqlite` → `reference.db` (`vendor_overlay` table)
//!
//! The `*Json`/`*Toml` implementations are left in place during this
//! migration rather than deleted outright — see each `sqlite_*` module's
//! doc comment for what still writes the file each one used to read.
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
mod sqlite_reference_repository;
mod sqlite_statistics_repository;
mod sqlite_vendor_repository;
mod statistics_repository;
mod vendor_repository;

pub use inventory_repository::InventoryRepositoryJson;
pub use market_repository::{MarketRepositoryJson, REFRESH_HISTORY_KEY};
pub use reference_repository::ReferenceRepositoryJson;
pub use settings_repository::SettingsRepositoryToml;
pub use sqlite_reference_repository::{REFERENCE_DB_PATH, ReferenceRepositorySqlite};
pub use sqlite_statistics_repository::{MARKET_DB_PATH, StatisticsRepositorySqlite};
pub use sqlite_vendor_repository::VendorRepositorySqlite;
pub use statistics_repository::StatisticsRepositoryJson;
pub use vendor_repository::VendorRepositoryToml;
