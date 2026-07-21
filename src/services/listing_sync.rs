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
//! Note: unlike `InventoryImportService` and `SetAnalysisService`, this one doesn't
//! reach presentation-free domain code — `fetch_user_listings` and
//! `build_priced_candidates` already print their own progress (`tsprintln!`/`print_info`
//! calls baked into those functions, same as before this service existed). This service
//! doesn't add any *new* printing, it just stops duplicating the call sequence; fully
//! silencing those two is a separate, larger change if it's ever wanted.
//!
//! Bigger wrinkle worth being upfront about: `fetch_user_listings`, `build_priced_candidates`,
//! `sort_candidates`, `load_blacklist`/`load_keeplist`, and `load_credentials` all physically
//! live under `cli/` (`cli::helpers`, `cli::pricing`, `cli::config_io`) even though none of
//! them are presentation logic — they just historically landed there. That means this service
//! calls back into `cli::*`, which inverts the "services depend on domain modules, cli depends
//! on services" layering this module's doc comment describes. It compiles fine (Rust doesn't
//! forbid circular item references within one crate), but it's a smell: the honest fix is
//! relocating those five functions out of `cli/` into `mapping`/`pricing`/a new home, not
//! routing around it here. Flagging it rather than quietly accepting it.

use std::collections::HashMap;

use crate::mapping::{BuildParentMap, BuildRequirements};
use crate::models::{BlacklistConfig, KeepConfig, MappedItem, WfcdItem, WfmItem};
use crate::wfm_client::{Credentials, Order as OwnedOrder, WfmClient};
use crate::{AppResult, cli, cli::ListingKey, pricing};

/// Everything ready for `process_candidates` to start its interactive loop: an
/// authenticated client, the account's current listings, and priced/sorted/filtered
/// trade candidates.
pub struct ListingSync {
    pub username: String,
    pub wfm_client: WfmClient,
    pub existing_listings_map: HashMap<ListingKey, Vec<OwnedOrder>>,
    pub active_slots_count: usize,
    /// How many tradeable candidates were identified before pricing — the number the CLI
    /// used to announce with "Identified N tradeable high-value candidates."
    pub candidate_count: usize,
    pub endo_rate: f64,
    pub priced_candidates: Vec<(MappedItem, f64, f64, u32, f64)>,
    pub upgrade_suggestions: Vec<(String, f64, u32, u32, f64)>,
    pub blacklist_set: BlacklistConfig,
    pub keeplist: KeepConfig,
}

/// Stateless application service (aside from the network calls it makes on the caller's
/// behalf): authenticates against WFM, fetches the account's current listings, and
/// builds/prices/sorts the full trade-candidate list.
pub struct ListingSyncService;

impl ListingSyncService {
    /// # Errors
    /// Returns an error if credentials are missing, WFM authentication fails, or any of
    /// the network/file-I/O calls this coordinates fail.
    pub async fn sync(
        mapped_items: Vec<MappedItem>,
        parent_map: &BuildParentMap,
        requirements: &BuildRequirements,
        wfcd_by_ref: &HashMap<String, WfcdItem>,
        wfm_by_name: &HashMap<String, WfmItem>,
        min_price: Option<f64>,
    ) -> AppResult<ListingSync> {
        let (email, password) = cli::load_credentials()?;
        let creds = Credentials::new(&email, &password, Credentials::generate_device_id());
        let wfm_client = WfmClient::from_credentials(creds).await?;
        let username = wfm_client.get_username().await?;

        let (user_listings, existing_listings_map) = cli::fetch_user_listings(&wfm_client).await?;
        let active_slots_count = user_listings.len();

        let candidates = pricing::filter_candidates(mapped_items, parent_map);
        let candidate_count = candidates.len();

        let endo_rate = pricing::derive_endo_to_plat_from_mods().await;

        let (priced_candidates, upgrade_suggestions) = cli::build_priced_candidates(
            candidates,
            endo_rate,
            parent_map,
            requirements,
            wfcd_by_ref,
            wfm_by_name,
            &cli::LiveStatsSource,
        )
        .await;

        let priced_candidates = cli::sort_candidates(priced_candidates, &existing_listings_map);
        let priced_candidates: Vec<_> = if let Some(min) = min_price {
            priced_candidates
                .into_iter()
                .filter(|c| c.1 >= min)
                .collect()
        } else {
            priced_candidates
        };

        let blacklist_set = cli::load_blacklist()?;
        let keeplist = cli::load_keeplist()?;

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
