//! SQLite-backed `VendorRepository`, replacing `VendorRepositoryToml` per
//! Phase 3 of the architecture plan. Shares `reference.db` with
//! `ReferenceRepositorySqlite` (same file, its own `vendor_overlay` table)
//! since both hold curated, slowly-changing game/vendor knowledge per the
//! plan's `reference.db` description.
//!
//! `config/vendors.toml` (`config::VENDORS_CONFIG_FILE`, via
//! `vendor::metadata::load_vendor_metadata`) remains the source of truth
//! for hand-editing; `VendorAnalysisService::load_vendors` mirrors it into
//! this table after each fetch, same incremental approach as
//! `ReferenceRepositorySqlite`.

use super::sqlite_reference_repository::REFERENCE_DB_PATH;
use super::traits::{RepositoryError, VendorRepository};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Mutex;

pub struct VendorRepositorySqlite<V> {
    conn: Mutex<Connection>,
    _marker: PhantomData<V>,
}

impl<V> VendorRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    /// Opens (creating if needed) `cache/reference.db` and ensures the
    /// `vendor_overlay` table exists.
    ///
    /// # Errors
    /// Returns an error if the database file can't be opened/created or the
    /// schema can't be initialized.
    pub fn open_default() -> Result<Self, RepositoryError> {
        Self::open(REFERENCE_DB_PATH)
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
            "CREATE TABLE IF NOT EXISTS vendor_overlay (
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
}

impl<V> VendorRepository for VendorRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    type Key = String;
    type Record = V;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("vendor overlay db lock poisoned".into()))?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM vendor_overlay WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        match raw {
            Some(raw) => Ok(serde_json::from_str(&raw)?),
            None => Err(RepositoryError::NotFound(format!(
                "no vendors.toml entry for '{key}'"
            ))),
        }
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        let raw = serde_json::to_string(&record)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("vendor overlay db lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO vendor_overlay (key, value_json, updated_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            params![key, raw],
        )
        .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(())
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("vendor overlay db lock poisoned".into()))?;
        let mut stmt = conn
            .prepare("SELECT key FROM vendor_overlay ORDER BY key")
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

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Fixture {
        name: String,
        value: u32,
    }

    fn scratch_repo() -> VendorRepositorySqlite<Fixture> {
        VendorRepositorySqlite::open(":memory:").expect("open failed")
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
        repo.upsert("widget_key".to_string(), record.clone())
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
}
