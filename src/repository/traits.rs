//! Repository trait definitions (Phase 2).
//!
//! Storage-agnostic on purpose: the initial implementations wrap the
//! existing JSON/TOML files this crate already reads/writes (see each
//! `*_repository.rs`), and Phase 3 swaps those implementations for
//! SQLite-backed ones without changing these signatures — so
//! `services::*` never has to change when storage does.

use std::fmt;

/// Error type shared across repository implementations. Concrete repos
/// map their underlying storage errors (JSON/TOML parse failures today,
/// `rusqlite`/`sqlx` errors post-Phase-3) into this so callers never need
/// to know which backend is in use.
#[derive(Debug)]
pub enum RepositoryError {
    NotFound(String),
    Validation(String),
    Backend(String),
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepositoryError::NotFound(msg) => write!(f, "not found: {msg}"),
            RepositoryError::Validation(msg) => write!(f, "validation error: {msg}"),
            RepositoryError::Backend(msg) => write!(f, "backend error: {msg}"),
        }
    }
}

impl std::error::Error for RepositoryError {}

impl From<String> for RepositoryError {
    /// Several existing loaders in this crate (`vendor::raw`) already
    /// return `Result<_, String>`. This lets repository impls propagate
    /// them with `?` instead of hand-wrapping every call site.
    fn from(msg: String) -> Self {
        RepositoryError::Backend(msg)
    }
}

impl From<std::io::Error> for RepositoryError {
    fn from(e: std::io::Error) -> Self {
        RepositoryError::Backend(e.to_string())
    }
}

impl From<serde_json::Error> for RepositoryError {
    fn from(e: serde_json::Error) -> Self {
        RepositoryError::Backend(e.to_string())
    }
}

impl From<toml::ser::Error> for RepositoryError {
    fn from(e: toml::ser::Error) -> Self {
        RepositoryError::Backend(e.to_string())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for RepositoryError {
    /// Several existing loaders (e.g. `vendor::metadata::load_vendor_metadata`)
    /// return `crate::AppResult<T>`. This lets repository impls propagate
    /// them with `?` instead of hand-wrapping every call site.
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        RepositoryError::Backend(e.to_string())
    }
}

/// Market statistics: per-item stats currently under `cache/statistics/`.
/// Backed by `market.db` from Phase 3 onward.
pub trait StatisticsRepository {
    type Key;
    type Record;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError>;
    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError>;
    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError>;
}

/// Market knowledge beyond raw statistics: refresh history (currently
/// `cache/metadata_cache.json`), volatility/shock detection state, price
/// models. Backed by `market.db`.
pub trait MarketRepository {
    type Key;
    type Record;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError>;
    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError>;
    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError>;
}

/// Curated, version-controlled game knowledge: vendors, vendor
/// inventories, item metadata, blueprint relationships, aliases. Backed
/// by `reference.db`.
pub trait ReferenceRepository {
    type Key;
    type Record;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError>;
    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError>;
    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError>;
    fn remove(&mut self, key: &Self::Key) -> Result<(), RepositoryError>;
}

/// User-owned inventory data. Backed by `profile.db`.
pub trait InventoryRepository {
    type Key;
    type Record;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError>;
    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError>;
    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError>;
    fn remove(&mut self, key: &Self::Key) -> Result<(), RepositoryError>;
}

/// Per-vendor *user* overlay data: `config/vendors.toml` (location,
/// group, `cost_mode`, exclusion, curation flags). Distinct from
/// `ReferenceRepository`, which owns the raw parsed vendor catalog
/// itself (`cache/vendors_raw_cache.json`).
pub trait VendorRepository {
    type Key;
    type Record;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError>;
    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError>;
    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError>;
}

/// User settings/auth/profile-level configuration. Backed by
/// `profile.db`; highest backup priority, never regenerated.
pub trait SettingsRepository {
    type Record;

    fn load(&self) -> Result<Self::Record, RepositoryError>;
    fn save(&mut self, record: Self::Record) -> Result<(), RepositoryError>;
}
