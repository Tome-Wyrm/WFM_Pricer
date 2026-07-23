//! `ListingSyncService` — everything `--` (no subcommand, the default interactive
//! session) needs before it can start asking the user per-item questions: authenticate,
//! fetch the account's current WFM listings, price and rank every tradeable candidate
//! against them.
//!
//! Extracted from `cli::sell::run_cli`, which used to do this setup inline before
//! dropping into `process_candidates`' interactive per-item loop. That loop (prompting
//! "list it? skip it? blacklist it?" for each candidate, one at a time) is genuinely
//! interactive and stays in `cli::sell` — it doesn't fit the "service returns data,
//! caller prints it" shape the way `SetAnalysisService` did, because the CLI needs to
//! read a decision from the user *between* processing each item, not after all of them.
//! What *can* be pulled out cleanly is everything that runs unconditionally before that
//! loop starts, which is what this service is.
//!
//! Note: `fetch_user_listings` and `build_priced_candidates` still print their own
//! progress (`tsprintln!` calls baked into those functions). This service doesn't add
//! any *new* printing, it just stops duplicating the call sequence; fully silencing
//! those two is a separate, larger change if it's ever wanted.
//!
//! Previously this service had to call back into `cli::*` for `fetch_user_listings`,
//! `build_priced_candidates`, `sort_candidates`, `load_blacklist`/`load_keeplist`, and
//! `load_credentials`, even though none of them are presentation logic — they'd just
//! historically landed under `cli/`. That's been fixed (Architecture Evolution Plan
//! Phase 1.5): `load_credentials`/`fetch_user_listings`/`ListingKey` moved to
//! `wfm_client`, `LiveStatsSource`/`build_priced_candidates`/`sort_candidates` moved to
//! top-level `pricing`, and `load_blacklist`/`load_keeplist` moved to a new top-level
//! `config_io` module. This service now depends only on domain modules, not `cli`.

use std::collections::HashMap;

use crate::mapping::{BuildParentMap, BuildRequirements};
use crate::models::{BlacklistConfig, KeepConfig, MappedItem, WfcdItem, WfmItem};
use crate::wfm_client::{Credentials, ListingKey, Order as OwnedOrder, WfmClient};
use crate::{AppResult, config_io, pricing};

/// Everything ready for `process_candidates` to start its interactive loop: an
/// authenticated client, the account's current listings, and priced/sorted/filtered
/// trade candidates.
pub(crate) struct ListingSync {
    pub(crate) username: String,
    pub(crate) wfm_client: WfmClient,
    pub(crate) existing_listings_map: HashMap<ListingKey, Vec<OwnedOrder>>,
    pub(crate) active_slots_count: usize,
    /// How many tradeable candidates were identified before pricing — the number the CLI
    /// used to announce with "Identified N tradeable high-value candidates."
    pub(crate) candidate_count: usize,
    pub(crate) endo_rate: f64,
    pub(crate) priced_candidates: Vec<(MappedItem, f64, f64, u32, f64)>,
    pub(crate) upgrade_suggestions: Vec<(String, f64, u32, u32, f64)>,
    pub(crate) blacklist_set: BlacklistConfig,
    pub(crate) keeplist: KeepConfig,
}

/// Stateless application service (aside from the network calls it makes on the caller's
/// behalf): authenticates against WFM, fetches the account's current listings, and
/// builds/prices/sorts the full trade-candidate list.
pub(crate) struct ListingSyncService;

impl ListingSyncService {
    /// # Errors
    /// Returns an error if credentials are missing, WFM authentication fails, or any of
    /// the network/file-I/O calls this coordinates fail.
    pub(crate) async fn sync(
        mapped_items: Vec<MappedItem>,
        parent_map: &BuildParentMap,
        requirements: &BuildRequirements,
        wfcd_by_ref: &HashMap<String, WfcdItem>,
        wfm_by_name: &HashMap<String, WfmItem>,
        min_price: Option<f64>,
    ) -> AppResult<ListingSync> {
        let (email, password) = crate::wfm_client::load_credentials()?;
        let creds = Credentials::new(&email, &password, Credentials::generate_device_id());
        let wfm_client = WfmClient::from_credentials(creds).await?;
        let username = wfm_client.get_username().await?;

        let (user_listings, existing_listings_map) =
            crate::wfm_client::fetch_user_listings(&wfm_client).await?;
        let active_slots_count = user_listings.len();

        let candidates = pricing::filter_candidates(mapped_items, parent_map);
        let candidate_count = candidates.len();

        let endo_rate = pricing::derive_endo_to_plat_from_mods().await;

        let (priced_candidates, upgrade_suggestions) = pricing::build_priced_candidates(
            candidates,
            endo_rate,
            parent_map,
            requirements,
            wfcd_by_ref,
            wfm_by_name,
            &pricing::LiveStatsSource,
        )
        .await;

        let priced_candidates = pricing::sort_candidates(priced_candidates, &existing_listings_map);
        let priced_candidates: Vec<_> = if let Some(min) = min_price {
            priced_candidates
                .into_iter()
                .filter(|c| c.1 >= min)
                .collect()
        } else {
            priced_candidates
        };

        let blacklist_set = config_io::load_blacklist()?;
        let keeplist = config_io::load_keeplist()?;

        Ok(ListingSync {
            username,
            wfm_client,
            existing_listings_map,
            active_slots_count,
            candidate_count,
            endo_rate,
            priced_candidates,
            upgrade_suggestions,
            blacklist_set,
            keeplist,
        })
    }
}
