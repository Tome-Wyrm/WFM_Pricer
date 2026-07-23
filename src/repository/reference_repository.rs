//! `ReferenceRepository` wrapping the existing parsed vendor cache
//! (`cache/vendors_raw_cache.json`, `config::VENDORS_RAW_CACHE_FILE`) via
//! `vendor::raw::{load_vendor_data, write_vendor_cache}`, keyed by
//! `RawVendor::key`.
//!
//! Deliberately reuses those existing functions rather than a generic
//! JSON store: `load_vendor_data`/`write_vendor_cache` are already the
//! tested read/write path for this file (see `vendor::raw`'s own tests),
//! so this repository is a thin adapter over them, not a second
//! implementation of the same file format.

use super::traits::{ReferenceRepository, RepositoryError};
use crate::vendor::raw::{RawVendor, load_vendor_data, write_vendor_cache};

pub struct ReferenceRepositoryJson;

impl ReferenceRepository for ReferenceRepositoryJson {
    type Key = String;
    type Record = RawVendor;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        let vendors = load_vendor_data()?;
        vendors
            .into_iter()
            .find(|v| &v.key == key)
            .ok_or_else(|| RepositoryError::NotFound(format!("vendor '{key}' not found")))
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        // Tolerate a missing cache file (nothing to load yet) rather than
        // treating "no cache yet" as an error on the very first upsert.
        let mut vendors = load_vendor_data().unwrap_or_default();
        vendors.retain(|v| v.key != key);
        vendors.push(record);
        write_vendor_cache(&vendors)?;
        Ok(())
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        let vendors = load_vendor_data()?;
        Ok(vendors.into_iter().map(|v| v.key).collect())
    }

    fn remove(&mut self, key: &Self::Key) -> Result<(), RepositoryError> {
        let mut vendors = load_vendor_data()?;
        let before = vendors.len();
        vendors.retain(|v| &v.key != key);
        if vendors.len() == before {
            return Err(RepositoryError::NotFound(format!(
                "vendor '{key}' not found"
            )));
        }
        write_vendor_cache(&vendors)?;
        Ok(())
    }
}
