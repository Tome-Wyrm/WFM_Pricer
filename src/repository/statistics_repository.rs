//! `StatisticsRepository` wrapping the existing `cache/statistics/`
//! directory (`config::STATISTICS_DIR`) — one JSON file per item slug.
//! Real backing type is `WfmStatsResponse`, currently read/written by
//! `pricing::fetch_statistics`.
//!
//! Kept generic over the record type (rather than hardcoding
//! `WfmStatsResponse` here) so this file doesn't need to depend on
//! `pricing`/`models` — `pricing::fetch_statistics` is the one that
//! names the concrete type when it opens `StatisticsRepositoryJson::<WfmStatsResponse>`.

use super::json_backend::JsonDirStore;
use super::traits::{RepositoryError, StatisticsRepository};
use serde::{Serialize, de::DeserializeOwned};

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

    /// Same as [`StatisticsRepository::upsert`] but takes `record` by
    /// reference instead of by value. `pricing::fetch_statistics` still
    /// needs to return the freshly-fetched value to its own caller after
    /// caching it, and requiring `V: Clone` purely to satisfy the
    /// trait's owned-`upsert` signature wasn't worth it — this avoids
    /// that bound entirely.
    ///
    /// # Errors
    /// Returns an error if the underlying storage can't be written.
    pub fn upsert_ref(&mut self, key: &str, record: &V) -> Result<(), RepositoryError> {
        self.store.upsert(key, record)
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
