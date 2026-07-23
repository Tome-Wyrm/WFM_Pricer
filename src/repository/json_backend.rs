//! Two small generic JSON storage helpers backing the initial (pre-SQLite)
//! repository implementations, matching the two on-disk shapes already
//! used elsewhere in this crate:
//!
//! - `JsonStore<K, V>` — one JSON file holding a `{key: record}` map.
//!   Mirrors the existing single-file caches (`cache/metadata_cache.json`
//!   via `mapping::cache`, `cache/full_items_cache.json`).
//! - `JsonDirStore<V>` — one JSON file per key inside a directory.
//!   Mirrors the existing `cache/statistics/` layout referenced in
//!   `config::STATISTICS_DIR`.
//!
//! Phase 3 replaces both with SQLite-backed storage behind the same
//! repository traits; nothing above the repository layer depends on
//! these types directly.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::hash::Hash;
use std::path::{Path, PathBuf};

use super::traits::RepositoryError;

/// Single-file `{key: record}` JSON map.
pub struct JsonStore<K, V> {
    path: PathBuf,
    data: HashMap<K, V>,
}

impl<K, V> JsonStore<K, V>
where
    K: Eq + Hash + Serialize + DeserializeOwned + Clone,
    V: Serialize + DeserializeOwned + Clone,
{
    /// Loads `path` if it exists (empty file = empty map), otherwise
    /// starts empty. The file is not created on disk until the first
    /// `upsert`/`remove`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, RepositoryError> {
        let path = path.as_ref().to_path_buf();
        let data = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                HashMap::new()
            } else {
                serde_json::from_str(&raw)?
            }
        } else {
            HashMap::new()
        };
        Ok(Self { path, data })
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.data.get(key)
    }

    pub fn upsert(&mut self, key: K, value: V) -> Result<(), RepositoryError> {
        self.data.insert(key, value);
        self.flush()
    }

    pub fn remove(&mut self, key: &K) -> Result<Option<V>, RepositoryError> {
        let removed = self.data.remove(key);
        self.flush()?;
        Ok(removed)
    }

    pub fn keys(&self) -> Vec<K> {
        self.data.keys().cloned().collect()
    }

    fn flush(&self) -> Result<(), RepositoryError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&self.data)?;
        fs::write(&self.path, raw)?;
        Ok(())
    }
}

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
