//! `MarketRepository` wrapping the existing WFCD refresh-history file
//! (`cache/metadata_cache.json`, `config::METADATA_FILE`), i.e. the
//! `CacheMetadata { wfcd_commit_hash, last_updated }` struct
//! `mapping::cache::update_caches` already reads/writes.
//!
//! This is a singleton file (not a per-key collection), so it's exposed
//! through the repository trait under one fixed key rather than
//! reinventing the on-disk shape. `mapping::cache::update_caches` keeps
//! writing the file directly for now — swapping its internal write for
//! this repository is a follow-up, not part of this pass, to avoid
//! changing cache-refresh behavior in the same patch that introduces the
//! trait.
use super::traits::{MarketRepository, RepositoryError};
use crate::mapping::cache::CacheMetadata;
use std::fs;
use std::path::Path;

/// Fixed key for the single refresh-history record this repository
/// currently exposes.
pub const REFRESH_HISTORY_KEY: &str = "wfcd_refresh";

pub struct MarketRepositoryJson;

impl MarketRepository for MarketRepositoryJson {
    type Key = String;
    type Record = CacheMetadata;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        if key != REFRESH_HISTORY_KEY {
            return Err(RepositoryError::NotFound(format!(
                "unknown market record key: '{key}'"
            )));
        }
        let path = Path::new(crate::config::METADATA_FILE);
        if !path.exists() {
            return Err(RepositoryError::NotFound(
                "no refresh history recorded yet".into(),
            ));
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        if key != REFRESH_HISTORY_KEY {
            return Err(RepositoryError::Validation(format!(
                "unknown market record key: '{key}'"
            )));
        }
        let raw = serde_json::to_string_pretty(&record)?;
        fs::write(crate::config::METADATA_FILE, raw)?;
        Ok(())
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        if Path::new(crate::config::METADATA_FILE).exists() {
            Ok(vec![REFRESH_HISTORY_KEY.to_string()])
        } else {
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These only exercise the wrong-key branches, which return before any
    // filesystem access — safe to run without touching the real
    // cache/metadata_cache.json. Right-key behavior isn't covered here
    // since it reads/writes that real file; covering it would need
    // METADATA_FILE to be injectable rather than a crate::config constant.

    #[test]
    fn get_with_wrong_key_is_not_found_without_touching_disk() {
        let repo = MarketRepositoryJson;
        match repo.get(&"not_the_real_key".to_string()) {
            Err(RepositoryError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn upsert_with_wrong_key_is_validation_error_without_touching_disk() {
        let mut repo = MarketRepositoryJson;
        let record = CacheMetadata {
            wfcd_commit_hash: "deadbeef".into(),
            last_updated: "irrelevant".into(),
        };
        match repo.upsert("not_the_real_key".to_string(), record) {
            Err(RepositoryError::Validation(_)) => {}
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
