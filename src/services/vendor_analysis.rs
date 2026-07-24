//! `VendorAnalysisService` — the non-interactive half of `vendor::run_vendor_cli`:
//! refreshing the vendor cache from the wiki and ranking a selection's offerings.
//!
//! The other half of `run_vendor_cli` — the location picker (`interactive_picker`) and
//! non-interactive path resolution (`resolve_path`), both walking a `LocTree` that's
//! private to `vendor::interactive` — stays exactly where it is. It's genuinely a
//! presentation concern (turning "which vendors did the user mean" into a selection),
//! not analysis, so it doesn't belong in a service. What *is* pure analysis — fetch,
//! build the mapped vendor cache, rank a set of vendors' offerings — lives here instead
//! of being called directly from `vendor::interactive::run_vendor_cli`.

use crate::repository::{
    ReferenceRepository, ReferenceRepositorySqlite, VendorRepository, VendorRepositorySqlite,
};
use crate::vendor::metadata::VendorMeta;
use crate::vendor::raw::RawVendor;
use crate::vendor::{MappedVendor, RankedOffering};
use crate::{AppResult, http, tseprintln, tsprintln, vendor};

pub(crate) struct VendorAnalysisService;

impl VendorAnalysisService {
    /// Refreshes the cached vendor data from the wiki (if stale) and returns every
    /// mapped vendor. Prints nothing — `fetch_and_cache_vendors`/`build_and_write_vendor_cache`
    /// already do their own progress reporting (revid-cache hit/miss, parse counts), same as
    /// before this service existed.
    ///
    /// # Errors
    /// Returns an error if the wiki fetch, Lua parse, or vendor-cache write fails.
    pub(crate) async fn load_vendors() -> AppResult<Vec<MappedVendor>> {
        vendor::fetch_and_cache_vendors(http::shared_client()).await?;
        let mapped = vendor::build_and_write_vendor_cache()?;

        // Phase 3: mirror what this fetch just wrote (cache/vendors_raw_cache.json,
        // config/vendors.toml) into reference.db via ReferenceRepository/
        // VendorRepository, so those repositories hold real data on every vendor
        // load instead of sitting unused. vendor::raw / vendor::metadata's own
        // JSON/TOML files stay the source of truth for the fetch pipeline itself
        // (fetch_and_cache_vendors / build_and_write_vendor_cache aren't touched)
        // — this keeps reference.db in sync alongside them, same incremental
        // migration approach Phase 3 used for market.db.
        match Self::sync_reference_db() {
            Ok((raw_count, overlay_count)) => {
                tsprintln!(
                    "Vendor repositories: {raw_count} raw vendor(s) cached, {overlay_count} triaged in vendors.toml."
                );
            }
            Err(e) => {
                tseprintln!("Warning: could not sync vendor repositories (reference.db): {e}");
            }
        }

        Ok(mapped)
    }

    /// Mirrors the raw vendor cache and the `vendors.toml` overlay into
    /// `reference.db`. Returns the number of raw vendors and overlay entries
    /// written, for the summary line in [`Self::load_vendors`].
    fn sync_reference_db() -> AppResult<(usize, usize)> {
        let raw_vendors = vendor::raw::load_vendor_data()?;
        let mut reference_repo = ReferenceRepositorySqlite::<RawVendor>::open_default()?;
        for v in &raw_vendors {
            reference_repo.upsert(v.key.clone(), v.clone())?;
        }

        let overlay = vendor::metadata::load_vendor_metadata()?;
        let mut vendor_repo = VendorRepositorySqlite::<VendorMeta>::open_default()?;
        for (key, meta) in &overlay {
            vendor_repo.upsert(key.clone(), meta.clone())?;
        }

        Ok((raw_vendors.len(), overlay.len()))
    }

    /// Ranks `vendors`' offerings by cost-efficiency score, applying the demand-floor and
    /// (optional) saturation-cap filters `rank_offerings` already implements.
    ///
    /// # Errors
    /// Returns an error if pricing data for an offering can't be fetched.
    pub(crate) async fn rank(
        vendors: &[MappedVendor],
        max_saturation: Option<f64>,
    ) -> AppResult<Vec<RankedOffering>> {
        vendor::rank_offerings(vendors, max_saturation).await
    }
}
