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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Fixture {
        name: String,
        value: u32,
    }

    /// Each test gets its own scratch directory under the OS temp dir
    /// rather than touching anything under the crate's own `cache/` —
    /// JsonDirStore takes an arbitrary path, so there's no need to fake
    /// a `config::STATISTICS_DIR` to exercise it. Cleaned up on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "wfm_pricer_repo_test_{label}_{n}_{:?}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn get_on_empty_store_is_not_found() {
        let dir = ScratchDir::new("get_empty");
        let store: JsonDirStore<Fixture> = JsonDirStore::new(&dir.0);
        match store.get("missing") {
            Err(RepositoryError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let dir = ScratchDir::new("round_trip");
        let store: JsonDirStore<Fixture> = JsonDirStore::new(&dir.0);
        let record = Fixture {
            name: "widget".into(),
            value: 42,
        };
        store.upsert("widget_key", &record).expect("upsert failed");
        let fetched = store.get("widget_key").expect("get failed");
        assert_eq!(fetched, record);
    }

    #[test]
    fn upsert_overwrites_existing_key() {
        let dir = ScratchDir::new("overwrite");
        let store: JsonDirStore<Fixture> = JsonDirStore::new(&dir.0);
        store
            .upsert(
                "key",
                &Fixture {
                    name: "first".into(),
                    value: 1,
                },
            )
            .expect("first upsert failed");
        store
            .upsert(
                "key",
                &Fixture {
                    name: "second".into(),
                    value: 2,
                },
            )
            .expect("second upsert failed");
        let fetched = store.get("key").expect("get failed");
        assert_eq!(
            fetched,
            Fixture {
                name: "second".into(),
                value: 2
            }
        );
    }

    #[test]
    fn list_keys_reflects_written_records() {
        let dir = ScratchDir::new("list_keys");
        let store: JsonDirStore<Fixture> = JsonDirStore::new(&dir.0);
        store
            .upsert(
                "alpha",
                &Fixture {
                    name: "a".into(),
                    value: 1,
                },
            )
            .expect("upsert alpha failed");
        store
            .upsert(
                "beta",
                &Fixture {
                    name: "b".into(),
                    value: 2,
                },
            )
            .expect("upsert beta failed");
        let mut keys = store.list_keys().expect("list_keys failed");
        keys.sort();
        assert_eq!(keys, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn list_keys_on_missing_directory_is_empty_not_error() {
        let dir = ScratchDir::new("missing_dir");
        // Deliberately never created — list_keys must treat "directory
        // doesn't exist yet" as "nothing cached yet", not an error.
        let store: JsonDirStore<Fixture> = JsonDirStore::new(&dir.0);
        assert_eq!(
            store.list_keys().expect("list_keys failed"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn keys_containing_path_separators_are_rejected() {
        let dir = ScratchDir::new("bad_key");
        let store: JsonDirStore<Fixture> = JsonDirStore::new(&dir.0);
        let record = Fixture {
            name: "x".into(),
            value: 0,
        };
        for bad_key in ["../escape", "a/b", "a\\b", "a.b", ""] {
            match store.upsert(bad_key, &record) {
                Err(RepositoryError::Validation(_)) => {}
                other => panic!("expected Validation error for key {bad_key:?}, got {other:?}"),
            }
        }
    }
}
