//! Phase 2 — Repository Layer
//!
//! Repositories sit between application services and storage. Per the
//! architecture plan: "initially repositories may wrap existing JSON or
//! in-memory implementations... later they become SQLite-backed without
//! affecting domain logic." That first JSON/TOML-wrapping pass has been
//! fully superseded — every trait below now has exactly one, SQLite-backed
//! implementation, across the three databases from the architecture plan:
//!
//! - `StatisticsRepositorySqlite`, `MarketRepositorySqlite` → `market.db`
//! - `ReferenceRepositorySqlite`, `VendorRepositorySqlite` → `reference.db`
//! - `InventoryRepositorySqlite`, `SettingsRepositorySqlite` → `profile.db`
//!
//! (Phase 1 gap cleanup: the old `*Json`/`*Toml` stub implementations —
//! `StatisticsRepositoryJson`, `MarketRepositoryJson`, `ReferenceRepositoryJson`,
//! `VendorRepositoryToml`, plus the never-wired-up `SettingsRepositoryToml`
//! stub file — were dead code, referenced only by their own tests and by
//! doc comments in the `sqlite_*` modules that replaced them. Removed rather
//! than left in place, since nothing outside this module depended on them
//! and keeping unused storage backends around invites drift.)
//!
//! Acceptance criteria this module works toward:
//! - Business logic does not execute SQL/file IO directly.
//! - Storage implementation is replaceable behind these traits.
//! - Validation occurs at repository boundaries.

mod traits;

pub use traits::{
    InventoryRepository, MarketRepository, ReferenceRepository, RepositoryError,
    SettingsRepository, StatisticsRepository, VendorRepository,
};

mod sqlite_inventory_repository;
mod sqlite_market_repository;
mod sqlite_reference_repository;
mod sqlite_settings_repository;
mod sqlite_statistics_repository;
mod sqlite_vendor_repository;

pub use sqlite_inventory_repository::{InventoryRepositorySqlite, PROFILE_DB_PATH};
pub use sqlite_market_repository::{MarketRepositorySqlite, REFRESH_HISTORY_KEY};
pub use sqlite_reference_repository::{REFERENCE_DB_PATH, ReferenceRepositorySqlite};
pub use sqlite_settings_repository::{DEFAULT_SETTINGS_KEY, SettingsRepositorySqlite};
pub use sqlite_statistics_repository::{MARKET_DB_PATH, StatisticsRepositorySqlite};
pub use sqlite_vendor_repository::VendorRepositorySqlite;
