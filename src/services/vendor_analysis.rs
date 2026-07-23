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
    ReferenceRepository, ReferenceRepositoryJson, VendorRepository, VendorRepositoryToml,
};
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

        // Phase 2 cleanup: read the cache this fetch just wrote back through
        // ReferenceRepository/VendorRepository, so those repositories are
        // exercised on every real vendor load instead of sitting unused.
        // build_and_write_vendor_cache's own read/parse of these same files
        // stays untouched — this is a read-only summary alongside it, not a
        // replacement, until matching.rs itself is migrated onto the
        // repository layer.
        let reference_repo = ReferenceRepositoryJson;
        let vendor_repo = VendorRepositoryToml;
        match (reference_repo.list_keys(), vendor_repo.list_keys()) {
            (Ok(raw_keys), Ok(overlay_keys)) => {
                tsprintln!(
                    "Vendor repositories: {} raw vendor(s) cached, {} triaged in vendors.toml.",
                    raw_keys.len(),
                    overlay_keys.len()
                );
            }
            (Err(e), _) | (_, Err(e)) => {
                tseprintln!("Warning: could not read vendor repositories for summary: {e}");
            }
        }

        Ok(mapped)
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
