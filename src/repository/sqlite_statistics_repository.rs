//! SQLite-backed `StatisticsRepository`, replacing `StatisticsRepositoryJson`
//! per Phase 3 of the architecture plan. Backs onto `market.db`: a single
//! `statistics` table storing each item's serialized record (e.g.
//! `WfmStatsResponse`) as a JSON blob keyed by slug, with `updated_at` used
//! for the 24h freshness check `pricing::fetch_statistics` previously did
//! against the JSON file's own mtime.
//!
//! Keeping the record itself as a JSON blob (rather than exploding
//! `WfmStatsResponse` into normalized columns) is deliberate for this first
//! Phase 3 slice: it preserves the exact `get`/`upsert`/`list_keys` contract
//! `StatisticsRepositoryJson` had, so callers don't need to change beyond
//! swapping which repository they open. Normalizing the 90-day price
//! history into real columns is future work once query needs (e.g. "prices
//! in date range") actually require it.

use super::traits::{RepositoryError, StatisticsRepository};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Mutex;

/// Default location for the market knowledge database (see architecture
/// plan, Phase 3 — "market.db").
pub const MARKET_DB_PATH: &str = "cache/market.db";

pub struct StatisticsRepositorySqlite<V> {
    conn: Mutex<Connection>,
    _marker: PhantomData<V>,
}

impl<V> StatisticsRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    /// Opens (creating if needed) `cache/market.db` and ensures the
    /// `statistics` table exists.
    ///
    /// # Errors
    /// Returns an error if the database file can't be opened/created or the
    /// schema can't be initialized.
    pub fn open_default() -> Result<Self, RepositoryError> {
        Self::open(MARKET_DB_PATH)
    }

    /// # Errors
    /// Returns an error if the database file can't be opened/created or the
    /// schema can't be initialized.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, RepositoryError> {
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(path.as_ref()).map_err(|e| RepositoryError::Backend(e.to_string()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS statistics (
                key        TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );",
        )
        .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
            _marker: PhantomData,
        })
    }

    /// Same rationale as `StatisticsRepositoryJson::upsert_ref`: lets
    /// `pricing::fetch_statistics` cache a value without giving up ownership
    /// of it, avoiding a `V: Clone` bound purely to satisfy the trait's
    /// owned-`upsert` signature.
    ///
    /// # Errors
    /// Returns an error if the underlying storage can't be written.
    pub fn upsert_ref(&mut self, key: &str, record: &V) -> Result<(), RepositoryError> {
        let raw = serde_json::to_string(record)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("statistics db lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO statistics (key, value_json, updated_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            params![key, raw],
        )
        .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Whether `key` has a record written within the last 24 hours.
    /// Replaces the old JSON-file-mtime check `fetch_statistics` used to do
    /// directly against `cache/statistics/{slug}.json`.
    ///
    /// # Errors
    /// Returns an error if the underlying storage can't be read.
    pub fn is_fresh(&self, key: &str) -> Result<bool, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("statistics db lock poisoned".into()))?;
        let fresh: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM statistics
                 WHERE key = ?1
                   AND updated_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day')",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(fresh.is_some())
    }
}

impl<V> StatisticsRepository for StatisticsRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    type Key = String;
    type Record = V;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("statistics db lock poisoned".into()))?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM statistics WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        match raw {
            Some(raw) => Ok(serde_json::from_str(&raw)?),
            None => Err(RepositoryError::NotFound(format!(
                "no statistics cached for '{key}'"
            ))),
        }
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        self.upsert_ref(&key, &record)
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("statistics db lock poisoned".into()))?;
        let mut stmt = conn
            .prepare("SELECT key FROM statistics ORDER BY key")
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        let keys = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| RepositoryError::Backend(e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Fixture {
        name: String,
        value: u32,
    }

    fn scratch_repo() -> StatisticsRepositorySqlite<Fixture> {
        StatisticsRepositorySqlite::open(":memory:").expect("open failed")
    }

    #[test]
    fn get_on_empty_store_is_not_found() {
        let repo = scratch_repo();
        match repo.get(&"missing".to_string()) {
            Err(RepositoryError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let mut repo = scratch_repo();
        let record = Fixture {
            name: "widget".into(),
            value: 42,
        };
        repo.upsert_ref("widget_key", &record)
            .expect("upsert failed");
        let fetched = repo.get(&"widget_key".to_string()).expect("get failed");
        assert_eq!(fetched, record);
    }

    #[test]
    fn upsert_overwrites_existing_key() {
        let mut repo = scratch_repo();
        repo.upsert(
            "key".to_string(),
            Fixture {
                name: "first".into(),
                value: 1,
            },
        )
        .expect("first upsert failed");
        repo.upsert(
            "key".to_string(),
            Fixture {
                name: "second".into(),
                value: 2,
            },
        )
        .expect("second upsert failed");
        let fetched = repo.get(&"key".to_string()).expect("get failed");
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
        let mut repo = scratch_repo();
        repo.upsert(
            "alpha".to_string(),
            Fixture {
                name: "a".into(),
                value: 1,
            },
        )
        .expect("upsert alpha failed");
        repo.upsert(
            "beta".to_string(),
            Fixture {
                name: "b".into(),
                value: 2,
            },
        )
        .expect("upsert beta failed");
        let mut keys = repo.list_keys().expect("list_keys failed");
        keys.sort();
        assert_eq!(keys, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn list_keys_on_fresh_db_is_empty_not_error() {
        let repo = scratch_repo();
        assert_eq!(
            repo.list_keys().expect("list_keys failed"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn is_fresh_is_false_for_unknown_key() {
        let repo = scratch_repo();
        assert!(!repo.is_fresh("missing").expect("is_fresh failed"));
    }

    #[test]
    fn is_fresh_is_true_immediately_after_upsert() {
        let mut repo = scratch_repo();
        repo.upsert_ref(
            "key",
            &Fixture {
                name: "x".into(),
                value: 1,
            },
        )
        .expect("upsert failed");
        assert!(repo.is_fresh("key").expect("is_fresh failed"));
    }
}
