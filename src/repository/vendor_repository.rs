//! `VendorRepository` wrapping the `config/vendors.toml` overlay
//! (`config::VENDORS_CONFIG_FILE`) — per-vendor `VendorMeta` (location,
//! group, `cost_mode`, `excluded`, `hand_curated`), read today via
//! `vendor::metadata::load_vendor_metadata`.
//!
//! `vendor::metadata::VendorConfig` only derives `Deserialize` (it's
//! load-only today), so this repository defines its own
//! `Serialize`-capable mirror shape for writes rather than editing that
//! struct's derives as a side effect of Phase 2. `VendorMeta` itself
//! already derives `Serialize`, so the mirror is just the wrapping
//! `{ vendor: HashMap<String, VendorMeta> }` table.

use super::traits::{RepositoryError, VendorRepository};
use crate::vendor::metadata::{VendorMeta, load_vendor_metadata};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;

#[derive(Serialize)]
struct VendorConfigOut {
    vendor: HashMap<String, VendorMeta>,
}

pub struct VendorRepositoryToml;

impl VendorRepository for VendorRepositoryToml {
    type Key = String;
    type Record = VendorMeta;

    fn get(&self, key: &Self::Key) -> Result<Self::Record, RepositoryError> {
        let all = load_vendor_metadata()?;
        all.get(key)
            .cloned()
            .ok_or_else(|| RepositoryError::NotFound(format!("no vendors.toml entry for '{key}'")))
    }

    fn upsert(&mut self, key: Self::Key, record: Self::Record) -> Result<(), RepositoryError> {
        let mut all = load_vendor_metadata()?;
        all.insert(key, record);
        let out = VendorConfigOut { vendor: all };
        let raw = toml::to_string(&out)?;
        fs::write(crate::config::VENDORS_CONFIG_FILE, raw)?;
        Ok(())
    }

    fn list_keys(&self) -> Result<Vec<Self::Key>, RepositoryError> {
        Ok(load_vendor_metadata()?.into_keys().collect())
    }
}
