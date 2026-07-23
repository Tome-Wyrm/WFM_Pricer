//! `StatisticsRepository` wrapping the existing `cache/statistics/`
//! directory (`config::STATISTICS_DIR`) — one JSON file per item slug,
//! matching the layout the `tests/fixtures/test_statistics` manifest
//! (referenced in `config.rs`) already assumes.
//!
//! Generic over the record type: this crate's actual per-item statistics
//! struct lives wherever `STATISTICS_DIR` is currently read from (not
//! part of this pass), so this repository stores/retrieves whatever
//! `Serialize + DeserializeOwned` type the caller wires up, keyed by
//! slug. Once that struct is identified, a type alias
//! (`pub type Statistics = StatisticsRepositoryJson<ItemStatistics>;`)
//! is the only extra step.

use super::json_backend::JsonDirStore;
use super::traits::{RepositoryError, StatisticsRepository};
use serde::{de::DeserializeOwned, Serialize};

pub struct StatisticsRepositoryJson<V> {
    store: JsonDirStore<V>,
}

impl<V> StatisticsRepositoryJson<V>
where
    V: Serialize + DeserializeOwned,
{
    /// Opens `cache/statistics/` (`config::STATISTICS_DIR`) as the
    /// backing directory. Swap for a SQLite-backed impl in Phase 3.
    #[must_use]
    pub fn open_default() -> Self {
        Self {
            store: JsonDirStore::new(crate::config::STATISTICS_DIR),
        }
    }
}

impl<V> StatisticsRepository for StatisticsRepositoryJson<V>
where
    V: Serialize + DeserializeOwned,
{
    type Key = String;
    type Record = V;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        self.store.get(key)
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        self.store.upsert(&key, &record)
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        self.store.list_keys()
    }
}
