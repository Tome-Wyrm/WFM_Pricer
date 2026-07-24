//! SQLite-backed `SettingsRepository` storing key-value or structured application state in `profile.db`.

use super::sqlite_inventory_repository::PROFILE_DB_PATH;
use super::traits::{RepositoryError, SettingsRepository};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Mutex;

pub const DEFAULT_SETTINGS_KEY: &str = "app_settings";

pub struct SettingsRepositorySqlite<V> {
    conn: Mutex<Connection>,
    _marker: PhantomData<V>,
}

impl<V> SettingsRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    /// Opens `cache/profile.db` and ensures `user_settings` table exists.
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
            "CREATE TABLE IF NOT EXISTS user_settings (
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

impl<V> SettingsRepository for SettingsRepositorySqlite<V>
where
    V: Serialize + DeserializeOwned,
{
    type Record = V;

    fn load(&self) -> Result<Self::Record, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("profile db lock poisoned".into()))?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM user_settings WHERE key = ?1",
                params![DEFAULT_SETTINGS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        match raw {
            Some(raw) => Ok(serde_json::from_str(&raw)?),
            None => Err(RepositoryError::NotFound(
                "settings record not found in profile.db".into(),
            )),
        }
    }

    fn save(&mut self, record: Self::Record) -> Result<(), RepositoryError> {
        let raw = serde_json::to_string(&record)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Backend("profile db lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO user_settings (key, value_json, updated_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            params![DEFAULT_SETTINGS_KEY, raw],
        )
        .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        Ok(())
    }
}
