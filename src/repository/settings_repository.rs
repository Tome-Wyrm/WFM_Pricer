//! `SettingsRepository` — not wired to a real source yet.
//!
//! No `config/settings.toml` (or equivalent) exists in the current tree.
//! `config_io.rs`'s `keeplist.toml`/`blacklist.toml` are closer to the
//! plan's "Watchlists / Saved filters" than to "Settings", so they
//! aren't reused here to avoid conflating the two. Wire this up once a
//! real settings file/struct exists, following the single-record pattern
//! `market_repository.rs` uses for `cache/metadata_cache.json`.

use super::traits::{RepositoryError, SettingsRepository};

pub struct SettingsRepositoryToml;

impl SettingsRepository for SettingsRepositoryToml {
    type Record = String;

    fn load(&self) -> Result<Self::Record, RepositoryError> {
        todo!("wire SettingsRepositoryToml once a settings file/struct exists")
    }

    fn save(&mut self, _record: Self::Record) -> Result<(), RepositoryError> {
        todo!("wire SettingsRepositoryToml once a settings file/struct exists")
    }
}
