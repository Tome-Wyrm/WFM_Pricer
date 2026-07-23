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

use super::traits::{InventoryRepository, RepositoryError};

pub struct InventoryRepositoryJson;

impl InventoryRepository for InventoryRepositoryJson {
    type Key = String;
    type Record = String;

    fn get(&self, _key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        todo!("wire InventoryRepositoryJson to ingestion.rs's inventory.json loader")
    }

    fn upsert(&mut self, _key: Self::Key, _record: Self::Record) -> Result<(), RepositoryError> {
        todo!("wire InventoryRepositoryJson to ingestion.rs's inventory.json loader")
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        todo!("wire InventoryRepositoryJson to ingestion.rs's inventory.json loader")
    }

    fn remove(&mut self, _key: &Self::Key) -> Result<(), RepositoryError> {
        todo!("wire InventoryRepositoryJson to ingestion.rs's inventory.json loader")
    }
}
