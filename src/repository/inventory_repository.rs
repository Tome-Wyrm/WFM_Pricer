//! `InventoryRepository` — not wired to a real source yet.
//!
//! Candidate source: `inventory.json` (the default path implied by
//! `Cli::inventory` in `main.rs`, `AlecaFrame`'s `lastData.dat` as
//! fallback). The actual loader/parser lives in `ingestion.rs`, which
//! wasn't part of this pass, so the record/key types below are
//! placeholders (`String`/`String`) rather than guessed-at real ones.
//! Wire this up the same way `reference_repository.rs` wraps
//! `vendor::raw` once `ingestion.rs`'s inventory type is available —
//! reuse its existing load/save functions rather than re-parsing
//! `inventory.json` a second way.
//!
//! Every method currently returns `RepositoryError::Backend` rather than
//! panicking (`todo!()`), so an accidental early call fails safely
//! instead of crashing the process.

use super::traits::{InventoryRepository, RepositoryError};

pub struct InventoryRepositoryJson;

impl InventoryRepository for InventoryRepositoryJson {
    type Key = String;
    type Record = String;

    /// # Errors
    /// Always returns `RepositoryError::Backend` — not wired up yet. See
    /// the module doc for what needs to land first.
    fn get(&self, _key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        Err(RepositoryError::Backend(
            "InventoryRepositoryJson is not wired to a real source yet \
             (needs ingestion.rs's inventory.json loader)"
                .into(),
        ))
    }

    /// # Errors
    /// Always returns `RepositoryError::Backend` — not wired up yet.
    fn upsert(&mut self, _key: Self::Key, _record: Self::Record) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(
            "InventoryRepositoryJson is not wired to a real source yet \
             (needs ingestion.rs's inventory.json loader)"
                .into(),
        ))
    }

    /// # Errors
    /// Always returns `RepositoryError::Backend` — not wired up yet.
    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        Err(RepositoryError::Backend(
            "InventoryRepositoryJson is not wired to a real source yet \
             (needs ingestion.rs's inventory.json loader)"
                .into(),
        ))
    }

    /// # Errors
    /// Always returns `RepositoryError::Backend` — not wired up yet.
    fn remove(&mut self, _key: &Self::Key) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(
            "InventoryRepositoryJson is not wired to a real source yet \
             (needs ingestion.rs's inventory.json loader)"
                .into(),
        ))
    }
}
