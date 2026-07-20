use std::collections::{HashMap, HashSet};
use std::error::Error;

use super::{
    BuildParentMap, BuildRequirements, Credentials, LiveStatsSource, MappedItem, SessionReport,
    WfcdItem, WfmClient, WfmItem, build_priced_candidates, derive_endo_to_plat_from_mods,
    fetch_user_listings, filter_candidates, load_blacklist, load_credentials, load_keeplist,
    print_header, print_info, print_upgrade_suggestions, process_candidates, sort_candidates,
    tsprintln, write_session_report,
};

/// Runs the interactive CLI loop.
///
/// # Errors
/// Returns an error if:
/// - Credentials are missing from environment.
/// - WFM client authentication fails.
/// - Network or file I/O operations fail.
/// - TOML or JSON serialization/deserialization fails.
#[allow(clippy::too_many_arguments)]
pub async fn run_cli(
    mapped_items: Vec<MappedItem>,
    parent_map: &BuildParentMap,
    mastered_set: &HashSet<String>,
    owned_built_set: &HashSet<String>,
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    min_price: Option<f64>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    print_header("Warframe.Market Advisor Session Init");

    let (email, password) = load_credentials()?;
    let creds = Credentials::new(&email, &password, Credentials::generate_device_id());
    let wfm_client = WfmClient::from_credentials(creds).await?;
    let username = wfm_client.get_username().await?;

    let (user_listings, existing_listings_map) = fetch_user_listings(&wfm_client).await?;
    let candidates = filter_candidates(mapped_items, parent_map);
    tsprintln!(
        "Identified {} tradeable high-value candidates.",
        candidates.len()
    );

    tsprintln!("Deriving dynamic Endo exchange rate from Ayatan prices...");
    let endo_rate = derive_endo_to_plat_from_mods().await;
    print_info(
        "Derived Endo Rate",
        &format!(
            "{:.5} plat/endo (or {:.1} plat per 1000 endo)",
            endo_rate,
            endo_rate * 1000.0
        ),
    );

    print_header("Trade Candidate Evaluation");
    tsprintln!("Fetching WFM pricing and volume stats dynamically for candidates...");

    let (priced_candidates, upgrade_suggestions) = build_priced_candidates(
        candidates,
        endo_rate,
        parent_map,
        requirements,
        wfcd_by_ref,
        wfm_by_name,
        &LiveStatsSource,
    )
    .await;
    print_upgrade_suggestions(&upgrade_suggestions);

    let priced_candidates = sort_candidates(priced_candidates, &existing_listings_map);
    let priced_candidates: Vec<_> = if let Some(min) = min_price {
        priced_candidates
            .into_iter()
            .filter(|c| c.1 >= min)
            .collect()
    } else {
        priced_candidates
    };

    let mut blacklist_set = load_blacklist()?;
    let mut keeplist = load_keeplist()?;
    let mut active_slots_count = user_listings.len();

    let session_items = process_candidates(
        priced_candidates,
        &existing_listings_map,
        &wfm_client,
        endo_rate,
        &mut blacklist_set,
        &mut keeplist,
        &mut active_slots_count,
        parent_map,
        mastered_set,
        owned_built_set,
    )
    .await?;

    let report = SessionReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        username,
        items_processed: session_items,
    };
    write_session_report(&report)?;

    print_header("WFM Pricer Session Completed Successfully");
    tsprintln!("Session report written to: 'session_report.json'");

    Ok(())
}
