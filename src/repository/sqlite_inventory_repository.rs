//! SQLite-backed `InventoryRepository` storing cached inventory records in `profile.db`.

use super::traits::{InventoryRepository, RepositoryError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Mutex;

pub const PROFILE_DB_PATH: &str = "cache/profile.db";

pub struct InventoryRepositorySqlite<V> {
    conn: Mutex<Connection>,
    _marker: PhantomData<V>,
}

impl<V> InventoryRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    /// Opens `cache/profile.db` and ensures `inventory_cache` table exists.
    ///
    /// # Errors
    /// Returns error if database connection or schema execution fails.
    pub fn open_default() -> Result<Self, RepositoryError> {
        Self::open(PROFILE_DB_PATH)
    }

    /// Opens database at given path.
    ///
    /// # Errors
    /// Returns error if path parent creation, connection open, or table creation fails.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, RepositoryError> {
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(path.as_ref()).map_err(|e| RepositoryError::Backend(e.to_string()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS inventory_cache (
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

impl<V> InventoryRepository for InventoryRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    type Key = String;
    type Record = V;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("profile db lock poisoned".into()))?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM inventory_cache WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        match raw {
            Some(raw) => Ok(serde_json::from_str(&raw)?),
            None => Err(RepositoryError::NotFound(format!(
                "inventory key '{key}' not found in profile.db"
            ))),
        }
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        let raw = serde_json::to_string(&record)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("profile db lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO inventory_cache (key, value_json, updated_at)
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
            .map_err(|_| RepositoryError::Backend("profile db lock poisoned".into()))?;
        let mut stmt = conn
            .prepare("SELECT key FROM inventory_cache ORDER BY key")
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
            .map_err(|_| RepositoryError::Backend("profile db lock poisoned".into()))?;
        conn.execute("DELETE FROM inventory_cache WHERE key = ?1", params![key])
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(())
    }
}
