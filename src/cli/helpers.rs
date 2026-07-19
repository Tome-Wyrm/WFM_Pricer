use std::collections::{HashMap, HashSet};
use std::error::Error;

use super::{
    BuildParentMap, BuildStatus, ListingKey, MappedItem, NoOpDecision, OwnedOrder,
    PRICE_TOLERANCE_PCT, WfmClient, get_build_status, tsprintln,
};

pub(crate) fn get_auto_keep(
    item: &MappedItem,
    parent_map: &BuildParentMap,
    mastered_set: &HashSet<String>,
    owned_built_set: &HashSet<String>,
) -> u32 {
    let status = get_build_status(&item.game_ref, parent_map, mastered_set, owned_built_set);
    u32::from(status == BuildStatus::NotBuilt)
}

/// Merges the manual keeplist reservation with the automatic build-status-driven floor.
/// Pulled out as its own pure function so the merge behavior is directly testable without
/// constructing a full `MappedItem`/`CandidateContext` pipeline.
pub(crate) fn resolve_keep_copies(manual_keep: u32, auto_keep: u32) -> u32 {
    std::cmp::max(manual_keep, auto_keep)
}

#[cfg(test)]
mod keep_quantity_tests {
    use super::*;

    #[test]
    fn manual_keep_used_when_no_auto_keep() {
        assert_eq!(resolve_keep_copies(2, 0), 2);
    }

    #[test]
    fn auto_keep_floor_applies_even_with_no_manual_entry() {
        assert_eq!(resolve_keep_copies(0, 1), 1);
    }

    #[test]
    fn manual_keep_wins_when_already_higher_than_auto() {
        assert_eq!(resolve_keep_copies(2, 1), 2);
    }

    #[test]
    fn auto_keep_floor_wins_over_a_lower_manual_entry() {
        // manual keeplist says 0 (or doesn't mention the item), but the build is still
        // "not built" — the auto floor of 1 must still win.
        assert_eq!(resolve_keep_copies(0, 1), 1);
    }
}

pub(crate) fn find_same_price_order<'a>(
    existing_listings_map: &'a HashMap<ListingKey, Vec<OwnedOrder>>,
    item_id: &str,
    rank: Option<u8>,
    price: u32,
) -> Option<&'a OwnedOrder> {
    existing_listings_map.iter().find_map(|(key, orders)| {
        if key.item_id == item_id && key.rank == rank {
            orders.iter().find(|o| o.platinum() == price)
        } else {
            None
        }
    })
}

pub(crate) fn resolve_action_choice(raw_input: &str) -> String {
    let trimmed = raw_input.trim().to_uppercase();
    if trimmed.is_empty() {
        "Y".to_string()
    } else {
        trimmed
    }
}

pub(crate) fn decide_no_op(
    suggested_price: u32,
    existing_price: u32,
    desired_total_qty: u32,
    existing_qty: u32,
) -> NoOpDecision {
    // suggested_price and PRICE_TOLERANCE_PCT are both non-negative and well within u32 range,
    // so the rounded result can never be negative or truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rounded_tolerance = (f64::from(suggested_price) * PRICE_TOLERANCE_PCT).round() as u32;
    let tolerance = std::cmp::max(1, rounded_tolerance);
    let price_matches = suggested_price.abs_diff(existing_price) <= tolerance;
    match (price_matches, desired_total_qty == existing_qty) {
        (true, true) => NoOpDecision::TrueNoOp,
        (true, false) => NoOpDecision::QuantitySyncOnly {
            new_quantity: desired_total_qty,
            keep_price: existing_price,
        },
        (false, _) => NoOpDecision::NeedsReview,
    }
}

pub(crate) fn quantity_default(
    is_already_listed: bool,
    listed_qty: u32,
    available_qty: u32,
) -> u32 {
    if is_already_listed {
        listed_qty + available_qty
    } else {
        available_qty
    }
}

pub(crate) fn ayatan_max_stars(slug: &str) -> (u8, u8) {
    match slug {
        "ayatan_anasa_sculpture" => (2, 2),
        "ayatan_ayr_sculpture" => (3, 0),
        "ayatan_chattraka_sculpture"
        | "ayatan_hemakara_sculpture"
        | "ayatan_piv_sculpture"
        | "ayatan_sah_sculpture"
        | "ayatan_valana_sculpture"
        | "ayatan_vaya_sculpture"
        | "ayatan_zambuka_sculpture" => (2, 1),
        "ayatan_kitha_sculpture" => (4, 1),
        "ayatan_orta_sculpture" => (3, 1),
        _ => (0, 0),
    }
}

pub(crate) fn print_header(title: &str) {
    tsprintln!(
        "\x1B[1;36m================================================================================\x1B[0m"
    );
    tsprintln!("\x1B[1;35m   {}   \x1B[0m", title.to_uppercase());
    tsprintln!(
        "\x1B[1;36m================================================================================\x1B[0m"
    );
}

pub(crate) fn print_info(label: &str, value: &str) {
    tsprintln!("\x1B[1;34m  {label:<25}\x1B[0m : \x1B[32m{value}\x1B[0m");
}

pub(crate) fn print_warning(msg: &str) {
    tsprintln!("\x1B[1;33m  [WARNING] {msg}\x1B[0m");
}

#[allow(dead_code)]
pub(crate) fn print_error_ui(msg: &str) {
    tsprintln!("\x1B[1;31m  [ERROR] {msg}\x1B[0m");
}

// ── Helper functions for `run_cli` ──────────────────────────────────────────

pub(crate) fn load_credentials() -> Result<(String, String), Box<dyn Error + Send + Sync>> {
    let email = std::env::var("WFM_EMAIL").unwrap_or_default();
    let password = std::env::var("WFM_PASSWORD").unwrap_or_default();
    if email.is_empty() || password.is_empty() {
        print_warning("WFM_EMAIL or WFM_PASSWORD not found in environment.");
        print_info(
            "Please supply them",
            "e.g., set WFM_EMAIL=email in environment or .env file.",
        );
        return Err("Missing credentials".into());
    }
    Ok((email, password))
}

pub(crate) async fn fetch_user_listings(
    wfm_client: &WfmClient,
) -> Result<(Vec<OwnedOrder>, HashMap<ListingKey, Vec<OwnedOrder>>), Box<dyn Error + Send + Sync>> {
    tsprintln!("Fetching your active listings from Warframe.Market...");
    let all_orders = wfm_client.my_orders().await?;
    let user_listings: Vec<OwnedOrder> =
        all_orders.into_iter().filter(OwnedOrder::is_sell).collect();
    let current_count = user_listings.len();
    print_info(
        "Active Listings on WFM",
        &format!("{current_count}/100 slots used"),
    );

    let mut map: HashMap<ListingKey, Vec<OwnedOrder>> = HashMap::new();
    for listing in &user_listings {
        map.entry(ListingKey {
            item_id: listing.item_id().to_string(),
            rank: listing.rank,
        })
        .or_default()
        .push(listing.clone());
    }
    Ok((user_listings, map))
}

/// Core set‑aggregation logic: takes a list of candidate items (which may include components),
/// the build maps, and a price map (slug → `wa_price`). Returns a new list of items where complete
/// sets are combined into a single `MappedItem` and their parts are removed.
///
/// Deliberately has no "part is worth too much relative to the set" guard: Warframe's Prime
/// part market routinely prices one component (usually the one from the rarest-tier relic)
/// above half the assembled set's price — that's normal relic-scarcity pricing, not a signal
/// that bundling is a bad trade. An earlier version of this function had such a guard and it
/// silently dropped entire sets (Corinth Prime, Akvasto Prime, Phantasma Prime, ...) whenever
/// their pricier component crossed the ratio.
///
/// This function is pure and synchronous, making it easy to unit test.

#[cfg(test)]
mod price_conflict_tests {
    use super::*;
    use crate::wfm_client::Order;

    #[test]
    fn does_not_match_across_different_ranks_at_same_price() {
        let mut map: HashMap<ListingKey, Vec<Order>> = HashMap::new();
        let item_id = "abc".to_string();

        let key0 = ListingKey {
            item_id: item_id.clone(),
            rank: Some(0),
        };
        let order0 = Order {
            id: "o0".into(),
            order_type: "sell".into(),
            platinum: 50,
            quantity: 1,
            item_id: item_id.clone(),
            visible: true,
            rank: Some(0),
            subtype: None,
        };
        map.entry(key0).or_default().push(order0);

        let key5 = ListingKey {
            item_id: item_id.clone(),
            rank: Some(5),
        };
        let order5 = Order {
            id: "o5".into(),
            order_type: "sell".into(),
            platinum: 50,
            quantity: 1,
            item_id: item_id.clone(),
            visible: true,
            rank: Some(5),
            subtype: None,
        };
        map.entry(key5).or_default().push(order5);

        let result = find_same_price_order(&map, &item_id, Some(5), 50);
        assert_eq!(result.map(|o| o.id()), Some("o5"));
        // Ensure it does not return the rank-0 order.
        assert_ne!(result.map(|o| o.id()), Some("o0"));
    }
}

#[cfg(test)]

mod quantity_default_tests {
    use super::*;

    #[test]
    fn restock_default_includes_already_listed_quantity() {
        assert_eq!(quantity_default(true, 100, 5), 105);
    }

    #[test]
    fn fresh_listing_default_is_just_available() {
        assert_eq!(quantity_default(false, 0, 5), 5);
    }
}

#[cfg(test)]

mod action_choice_tests {
    use super::*;

    #[test]
    fn empty_input_defaults_to_yes() {
        assert_eq!(resolve_action_choice("\n"), "Y");
        assert_eq!(resolve_action_choice(""), "Y");
    }

    #[test]
    fn explicit_choices_pass_through_uppercased() {
        assert_eq!(resolve_action_choice("n\n"), "N");
        assert_eq!(resolve_action_choice("x"), "X");
        assert_eq!(resolve_action_choice("k"), "K");
        assert_eq!(resolve_action_choice("b"), "B");
        assert_eq!(resolve_action_choice("y"), "Y");
    }
}

#[cfg(test)]

mod no_op_decision_tests {
    use super::*;

    #[test]
    fn stable_ayatan_star_is_a_true_noop() {
        // 100 owned, 100 listed, 1p suggested, 1p existing.
        assert!(matches!(
            decide_no_op(1, 1, 100, 100),
            NoOpDecision::TrueNoOp
        ));
    }

    #[test]
    fn restock_with_stable_price_is_quantity_sync_only() {
        // 105 owned (100 listed + 5 new), price unchanged at 1p.
        assert!(matches!(
            decide_no_op(1, 1, 105, 100),
            NoOpDecision::QuantitySyncOnly {
                new_quantity: 105,
                ..
            }
        ));
    }

    #[test]
    fn real_price_move_needs_review() {
        // existing listed at 40p, market now suggests 55p — well outside 3% tolerance.
        assert!(matches!(
            decide_no_op(55, 40, 10, 10),
            NoOpDecision::NeedsReview
        ));
    }

    #[test]
    fn small_drift_within_tolerance_is_still_noop() {
        // 41p existing vs 42p suggested on a price where 3% tolerance is >= 1.
        assert!(matches!(
            decide_no_op(42, 41, 10, 10),
            NoOpDecision::TrueNoOp
        ));
    }
}

#[cfg(test)]

mod threshold_calibration_tests {
    use crate::models::WfmStatsResponse;
    use crate::pricing::recent_volume;
    use std::fs;

    // Reuse the load_fixture helper from recent_volume_tests (copy it here or refer to it).
    fn load_fixture(name: &str) -> WfmStatsResponse {
        let path = format!("tests/fixtures/test_statistics/{name}.json");
        let raw = fs::read_to_string(&path).expect("fixture missing — see Task 0.1");
        serde_json::from_str(&raw).expect("fixture failed to parse")
    }

    #[test]
    fn calibration_set_separates_cleanly_on_volume_floor() {
        // Junk cases from Task 0.1 manifest
        let junk_cases = [
            ("vitality", 0u8),
            ("vitality", 10),
            ("steel_fiber", 0),
            ("steel_fiber", 10),
            ("arcane_ice", 0),
            ("arcane_ice", 5),
        ];

        // Real-demand cases (including the weakest one: archon_flow r10 at 24.2/day)
        let real_demand_cases = [
            ("archon_flow", 10u8),
            ("archon_flow", 0),
            ("archon_stretch", 0),
            ("archon_stretch", 10),
            ("primed_flow", 0),
            ("primed_flow", 10),
            ("primed_pressure_point", 0),
            ("primed_pressure_point", 10),
            ("molt_reconstruct", 0),
            ("molt_reconstruct", 5),
            ("molt_augmented", 0),
            ("molt_augmented", 5),
            ("arcane_energize", 0),
            ("arcane_energize", 5),
            ("arcane_persistence", 0),
            ("arcane_persistence", 5),
            ("primary_merciless", 0),
            ("primary_merciless", 5),
        ];

        // Now lives in config.rs — applied universally as of vendor-rank Phase F.
        use crate::config::MIN_DAILY_VOLUME;

        for (slug, rank) in junk_cases {
            let stats = load_fixture(slug);
            let (vol, _) = recent_volume(&stats, Some(rank), 30);
            let per_day = f64::from(vol) / 30.0;
            assert!(
                per_day < MIN_DAILY_VOLUME,
                "{slug} r{rank} should read as junk, got {per_day:.2}/day"
            );
        }

        for (slug, rank) in real_demand_cases {
            let stats = load_fixture(slug);
            let (vol, _) = recent_volume(&stats, Some(rank), 30);
            let per_day = f64::from(vol) / 30.0;
            assert!(
                per_day >= MIN_DAILY_VOLUME,
                "{slug} r{rank} should clear the demand floor, got {per_day:.2}/day"
            );
        }
    }
}

/// Title-cases a relic refinement for display (`"intact"` -> `"Intact"`). Both the live order
/// book's `subtype` field and `MappedItem::owned_subtype` come through lowercase — confirmed
/// against the real API (`GET /v2/orders/item/lith_o2_relic` returns `"subtype": "intact"`,
/// `"radiant"`, etc.) — so this is purely presentational.
pub(crate) fn capitalize_tier(tier: &str) -> String {
    let mut chars = tier.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}
