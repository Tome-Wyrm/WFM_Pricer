//! SQLite-backed `ReferenceRepository`, replacing `ReferenceRepositoryJson`
//! per Phase 3 of the architecture plan. Backs onto `reference.db`'s
//! `vendors_raw` table: each `RawVendor` stored as a JSON blob keyed by
//! vendor key (matching `RawVendor::key`), mirroring the same
//! JSON-blob-in-SQLite approach `StatisticsRepositorySqlite` uses for
//! `market.db` — normalizing offerings into real columns is Phase 4
//! (Reference Data System) work, once curated import/validation actually
//! needs to query them individually rather than round-trip the whole record.
//!
//! `vendor::raw::{load_vendor_data, write_vendor_cache}` (the
//! `cache/vendors_raw_cache.json` file) remain the source of truth for the
//! vendor fetch pipeline itself; `VendorAnalysisService::load_vendors`
//! mirrors what they just wrote into this repository afterward, same
//! incremental-migration approach Phase 3 used for `market.db`.

use super::traits::{ReferenceRepository, RepositoryError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Mutex;

/// Default location for the reference knowledge database (see architecture
/// plan, Phase 3 — "reference.db"). Shared with `VendorRepositorySqlite`,
/// which uses its own `vendor_overlay` table in the same file.
pub const REFERENCE_DB_PATH: &str = "cache/reference.db";

pub struct ReferenceRepositorySqlite<V> {
    conn: Mutex<Connection>,
    _marker: PhantomData<V>,
}

impl<V> ReferenceRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    /// Opens (creating if needed) `cache/reference.db` and ensures the
    /// `vendors_raw` table exists.
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
            "CREATE TABLE IF NOT EXISTS vendors_raw (
                key        TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );
             CREATE TABLE IF NOT EXISTS curated_vendor_offerings (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                vendor_key   TEXT NOT NULL,
                vendor_name  TEXT NOT NULL,
                location     TEXT,
                vendor_group TEXT,
                offering_name TEXT NOT NULL,
                category     TEXT NOT NULL,
                currency     TEXT NOT NULL,
                cost_amount  REAL NOT NULL,
                cost_mode    TEXT NOT NULL,
                wfm_slug     TEXT,
                target_rank  INTEGER,
                notes        TEXT,
                updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );",
        )
        .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
            _marker: PhantomData,
        })
    }

    /// Bulk imports validated spreadsheet rows into the `curated_vendor_offerings` table.
    ///
    /// # Errors
    /// Returns an error if database operations fail.
    pub fn import_spreadsheet_rows(
        &mut self,
        rows: &[crate::vendor::spreadsheet::VendorSpreadsheetRow],
    ) -> Result<usize, RepositoryError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("reference db lock poisoned".into()))?;
        let tx = conn.transaction().map_err(|e| RepositoryError::Backend(e.to_string()))?;

        // Clear previous imported rows
        tx.execute("DELETE FROM curated_vendor_offerings", [])
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;

        let mut count = 0;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO curated_vendor_offerings (
                        vendor_key, vendor_name, location, vendor_group, offering_name,
                        category, currency, cost_amount, cost_mode, wfm_slug, target_rank, notes
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )
                .map_err(|e| RepositoryError::Backend(e.to_string()))?;

            for row in rows {
                stmt.execute(params![
                    row.vendor_key,
                    row.vendor_name,
                    row.location,
                    row.group,
                    row.offering_name,
                    row.category,
                    row.currency,
                    row.cost_amount,
                    row.cost_mode,
                    row.wfm_slug,
                    row.target_rank,
                    row.notes,
                ])
                .map_err(|e| RepositoryError::Backend(e.to_string()))?;
                count += 1;
            }
        }

        tx.commit().map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(count)
    }
}

impl<V> ReferenceRepository for ReferenceRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    type Key = String;
    type Record = V;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("reference db lock poisoned".into()))?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM vendors_raw WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        match raw {
            Some(raw) => Ok(serde_json::from_str(&raw)?),
            None => Err(RepositoryError::NotFound(format!(
                "vendor '{key}' not found"
            ))),
        }
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        let raw = serde_json::to_string(&record)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("reference db lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO vendors_raw (key, value_json, updated_at)
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
            .map_err(|_| RepositoryError::Backend("reference db lock poisoned".into()))?;
        let mut stmt = conn
            .prepare("SELECT key FROM vendors_raw ORDER BY key")
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        let keys = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| RepositoryError::Backend(e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(keys)
    }

    fn remove(&mut self, key: &Self::Key) -> Result<(), RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("reference db lock poisoned".into()))?;
        let affected = conn
            .execute("DELETE FROM vendors_raw WHERE key = ?1", params![key])
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        if affected == 0 {
            return Err(RepositoryError::NotFound(format!(
                "vendor '{key}' not found"
            )));
        }
        Ok(())
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

    fn scratch_repo() -> ReferenceRepositorySqlite<Fixture> {
        ReferenceRepositorySqlite::open(":memory:").expect("open failed")
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
    fn remove_deletes_and_reports_not_found_on_second_call() {
        let mut repo = scratch_repo();
        repo.upsert(
            "key".to_string(),
            Fixture {
                name: "x".into(),
                value: 1,
            },
        )
        .expect("upsert failed");
        repo.remove(&"key".to_string()).expect("remove failed");
        match repo.remove(&"key".to_string()) {
            Err(RepositoryError::NotFound(_)) => {}
            other => panic!("expected NotFound on second remove, got {other:?}"),
        }
    }

    #[test]
    fn remove_on_missing_key_is_not_found() {
        let mut repo = scratch_repo();
        match repo.remove(&"missing".to_string()) {
            Err(RepositoryError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
