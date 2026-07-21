//! Central registry of on-disk paths and a handful of cross-cutting tuning constants
//! (`cache/`, `config/`, and per-file paths within each). No logic lives here — this is
//! deliberately just the "where does X live" reference for every other module's file I/O,
//! so a path never needs to be duplicated/hardcoded at a second call site.

pub const CONFIG_DIR: &str = "config";
pub const CACHE_DIR: &str = "cache";
pub const STATISTICS_DIR: &str = "cache/statistics";

pub const FULL_ITEMS_CACHE_FILE: &str = "cache/full_items_cache.json";
pub const METADATA_FILE: &str = "cache/metadata_cache.json";
pub const RELICS_CACHE_FILE: &str = "cache/relics_cache.json";
pub const WFCD_CACHE_FILE: &str = "cache/wfcd_all_cache.json";
pub const WFM_CACHE_FILE: &str = "cache/wfm_items_cache.json";

pub const KEEPLIST_FILE: &str = "config/keeplist.toml";
pub const BLACKLIST_FILE: &str = "config/blacklist.toml";
pub const SESSION_REPORT_FILE: &str = "config/session_report.json";
pub const VENDORS_CONFIG_FILE: &str = "config/vendors.toml";

pub const VENDORS_RAW_CACHE_FILE: &str = "cache/vendors_raw_cache.json";
pub const VENDORS_CACHE_FILE: &str = "cache/vendors_cache.json";
pub const VENDOR_REVID_FILE: &str = "cache/vendor_revid.json";

/// Minimum average daily trade volume (trailing 30 days) for an item to be worth a listing
/// slot / a vendor-rank ranking row. Calibrated against real WFM data (see
/// `tests/fixtures/test_statistics` manifest): every confirmed-junk sample (common ubiquitous
/// mods, an unused eidolon arcane) topped out at 3.1/day; every confirmed-real-demand sample,
/// including the weakest one tested, started at 24.2/day. This sits in that gap. Applies
/// identically at every rank — junk stays junk and real demand clears the bar at both unranked
/// and maxed; no rank-specific adjustment needed. Originally scoped to mods/arcanes only in
/// cli.rs; applied universally as of vendor-rank Phase F (no per-category floor for v1).
pub const MIN_DAILY_VOLUME: f64 = 9.0;
