//! `SettingsRepository` — not wired to a real source yet.
//!
//! No `config/settings.toml` (or equivalent) exists in the current tree.
//! `config_io.rs`'s `keeplist.toml`/`blacklist.toml` are closer to the
//! plan's "Watchlists / Saved filters" than to "Settings", so they
//! aren't reused here to avoid conflating the two. Wire this up once a
//! real settings file/struct exists, following the single-record pattern
//! `market_repository.rs` uses for `cache/metadata_cache.json`.
//!
//! Both methods currently return `RepositoryError::Backend` rather than
//! panicking (`todo!()`), so an accidental early call fails safely
//! instead of crashing the process.

use super::traits::{RepositoryError, SettingsRepository};

pub struct SettingsRepositoryToml;

impl SettingsRepository for SettingsRepositoryToml {
    type Record = String;

    /// # Errors
    /// Always returns `RepositoryError::Backend` — not wired up yet. See
    /// the module doc for what needs to land first.
    fn load(&self) -> Result<Self::Record, RepositoryError> {
        Err(RepositoryError::Backend(
            "SettingsRepositoryToml is not wired to a real source yet \
             (no settings file/struct exists in the tree yet)"
                .into(),
        ))
    }

    /// # Errors
    /// Always returns `RepositoryError::Backend` — not wired up yet.
    fn save(&mut self, _record: Self::Record) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(
            "SettingsRepositoryToml is not wired to a real source yet \
             (no settings file/struct exists in the tree yet)"
                .into(),
        ))
    }
}
