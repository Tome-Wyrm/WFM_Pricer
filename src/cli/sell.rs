use crate::AppResult;
use crate::services::ListingSyncService;
use std::collections::{HashMap, HashSet};

use super::{
    BuildParentMap, BuildRequirements, MappedItem, SessionReport, WfcdItem, WfmItem, print_header,
    print_info, print_upgrade_suggestions, process_candidates, tsprintln, write_session_report,
};

/// Runs the interactive CLI loop.
///
/// Setup (auth, fetching current listings, pricing/ranking every candidate) is handled by
/// `services::ListingSyncService` — this function announces progress around that one call,
/// then runs the interactive per-item loop (`process_candidates`) that has to stay here
/// since it prompts the user for a decision on each candidate as it goes, not after the
/// whole list is ready.
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
) -> AppResult<()> {
    print_header("Warframe.Market Advisor Session Init");

    let mut sync = ListingSyncService::sync(
        mapped_items,
        parent_map,
        requirements,
        wfcd_by_ref,
        wfm_by_name,
        min_price,
    )
    .await?;

    tsprintln!(
        "Identified {} tradeable high-value candidates.",
        sync.candidate_count
    );
    print_info(
        "Derived Endo Rate",
        &format!(
            "{:.5} plat/endo (or {:.1} plat per 1000 endo)",
            sync.endo_rate,
            sync.endo_rate * 1000.0
        ),
    );

    print_header("Trade Candidate Evaluation");
    print_upgrade_suggestions(&sync.upgrade_suggestions);

    let session_items = process_candidates(
        sync.priced_candidates,
        &sync.existing_listings_map,
        &sync.wfm_client,
        sync.endo_rate,
        &mut sync.blacklist_set,
        &mut sync.keeplist,
        &mut sync.active_slots_count,
        parent_map,
        mastered_set,
        owned_built_set,
    )
    .await?;

    let report = SessionReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        username: sync.username,
        items_processed: session_items,
    };
    write_session_report(&report)?;

    print_header("WFM Pricer Session Completed Successfully");
    tsprintln!("Session report written to: 'session_report.json'");

    Ok(())
}
