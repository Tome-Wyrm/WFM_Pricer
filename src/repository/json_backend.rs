//! Generic JSON storage helper backing the initial (pre-SQLite)
//! `StatisticsRepository` implementation, matching the on-disk shape
//! already used for `cache/statistics/` (`config::STATISTICS_DIR`): one
//! JSON file per key.
//!
//! `MarketRepositoryJson` reads/writes its single `cache/metadata_cache.json`
//! record directly rather than through a generic single-file store — it's
//! one fixed key, so a keyed map abstraction added a layer without buying
//! anything. If a second singleton-file repository shows up, revisit.
//!
//! Phase 3 replaces this with SQLite-backed storage behind the same
//! repository trait; nothing above the repository layer depends on this
//! type directly.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::{Path, PathBuf};

use super::traits::RepositoryError;

/// One JSON file per key, inside a directory (`{dir}/{key}.json`).
/// Keys are restricted to plain filename-safe strings.
pub struct JsonDirStore<V> {
    dir: PathBuf,
    _marker: std::marker::PhantomData<V>,
}

impl<V> JsonDirStore<V>
where
    V: Serialize + DeserializeOwned,
{
    #[must_use]
    pub fn new<P: AsRef<Path>>(dir: P) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
            _marker: std::marker::PhantomData,
        }
    }

    fn file_path(&self, key: &str) -> Result<PathBuf, RepositoryError> {
        if key.is_empty() || key.contains(['/', '\\', '.']) {
            return Err(RepositoryError::Validation(format!(
                "invalid statistics key: '{key}'"
            )));
        }
        Ok(self.dir.join(format!("{key}.json")))
    }

    pub fn get(&self, key: &str) -> Result<V, RepositoryError> {
        let path = self.file_path(key)?;
        if !path.exists() {
            return Err(RepositoryError::NotFound(format!(
                "no statistics cached for '{key}'"
            )));
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn upsert(&self, key: &str, value: &V) -> Result<(), RepositoryError> {
        let path = self.file_path(key)?;
        fs::create_dir_all(&self.dir)?;
        let raw = serde_json::to_string_pretty(value)?;
        fs::write(path, raw)?;
        Ok(())
    }

    pub fn list_keys(&self) -> Result<Vec<String>, RepositoryError> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut keys = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                keys.push(stem.to_string());
            }
        }
        Ok(keys)
    }
}
