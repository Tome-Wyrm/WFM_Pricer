use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tokio::time::{sleep, Duration};
use crate::wfm_client::{WfmClient, Credentials, CreateOrder, UpdateOrder, Order as OwnedOrder};
use crate::config::{KEEPLIST_FILE, BLACKLIST_FILE, MIN_DAILY_VOLUME};
use crate::mapping::{BuildParentMap, BuildRequirements, BuildStatus, get_build_status, resolve_set_item};
// Whole-module imports (as opposed to the specific-item imports above) for the
// check-sets feature, which needs several more mapping::/wfm_client:: items than are
// worth naming individually here.
use crate::mapping;
use crate::wfm_client;
use crate::models::{MappedItem, KeepConfig, KeepRule, BlacklistConfig, WfcdItem, WfmItem, WfmStatsResponse};
use crate::pricing::{
    calculate_saturation_ratio, calculate_weighted_average, derive_endo_to_plat_from_mods,
    fetch_statistics, get_ayatan_endo_yield, is_antique, get_fusion_cost_from_zero, recent_volume,
};
// Timestamped session logging: see src/logging.rs.
use crate::{tseprintln, tsprint, tsprintln};

/// How far a recalculated price can drift from the currently-listed price before we bother
/// showing it to the user. Keeps a continuously-recalculated weighted-average price from
/// triggering a re-prompt every run over noise (e.g. 41.3p -> 41.7p). Tune as needed —
/// percentage with a 1-plat floor so cheap items (1-2p Ayatan stars) aren't hypersensitive.
const PRICE_TOLERANCE_PCT: f64 = 0.03;
/// If a single required component's own market price exceeds this fraction of the assembled
/// Set's own market price, sell that component standalone instead of folding it into the Set
/// bundle — otherwise you're giving away a disproportionately valuable part inside a cheaper
/// bundle price.

enum NoOpDecision {
    TrueNoOp,
    QuantitySyncOnly { new_quantity: u32, keep_price: u32 },
    NeedsReview,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionReportItem {
    pub name: String,
    pub slug: String,
    pub price: u32,
    pub quantity: u32,
    pub rank: Option<u32>,
    pub action: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionReport {
    pub timestamp: String,
    pub username: String,
    pub items_processed: Vec<SessionReportItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ListingKey {
    item_id: String,
    rank: Option<u8>,
}

struct CandidateContext<'a> {
    existing_listings_map: &'a HashMap<ListingKey, Vec<OwnedOrder>>,
    wfm_client: &'a WfmClient,
    endo_rate: f64,
    blacklist_set: &'a mut BlacklistConfig,
    keeplist: &'a mut KeepConfig,
    active_slots_count: &'a mut usize,
    stdout: &'a mut io::Stdout,
    parent_map: &'a BuildParentMap,
    mastered_set: &'a HashSet<String>,
    owned_built_set: &'a HashSet<String>,
}

struct PrimedMod {
    name: &'static str,
    slug: &'static str,
}

struct PrimedPrice {
    name: &'static str,
    price: f64,
    volume: u32,
}

const PRIMED_MODS: &[PrimedMod] = &[
    PrimedMod {
        name: "Archon Continuity",
        slug: "archon_continuity",
    },
    PrimedMod {
        name: "Archon Flow",
        slug: "archon_flow",
    },
    PrimedMod {
        name: "Archon Intensify",
        slug: "archon_intensify",
    },
    PrimedMod {
        name: "Archon Stretch",
        slug: "archon_stretch",
    },
    PrimedMod {
        name: "Archon Vitality",
        slug: "archon_vitality",
    },
    PrimedMod {
        name: "Primed Ammo Chain",
        slug: "primed_ammo_chain",
    },
    PrimedMod {
        name: "Primed Ammo Stock",
        slug: "primed_ammo_stock",
    },
    PrimedMod {
        name: "Primed Animal Instinct",
        slug: "primed_animal_instinct",
    },
    PrimedMod {
        name: "Primed Bane of Corpus",
        slug: "primed_bane_of_corpus",
    },
    PrimedMod {
        name: "Primed Bane of Grineer",
        slug: "primed_bane_of_grineer",
    },
    PrimedMod {
        name: "Primed Bane of Infested",
        slug: "primed_bane_of_infested",
    },
    PrimedMod {
        name: "Primed Bane of Orokin ",
        slug: "primed_bane_of_corrupted",
    },
    PrimedMod {
        name: "Primed Bane of The Murmur",
        slug: "primed_bane_of_the_murmur",
    },
    PrimedMod {
        name: "Primed Charged Shell",
        slug: "primed_charged_shell",
    },
    PrimedMod {
        name: "Primed Chilling Grasp",
        slug: "primed_chilling_grasp",
    },
    PrimedMod {
        name: "Primed Cleanse Corpus",
        slug: "primed_cleanse_corpus",
    },
    PrimedMod {
        name: "Primed Cleanse Grineer",
        slug: "primed_cleanse_grineer",
    },
    PrimedMod {
        name: "Primed Cleanse Infested",
        slug: "primed_cleanse_infested",
    },
    PrimedMod {
        name: "Primed Cleanse Orokin ",
        slug: "primed_cleanse_corrupted",
    },
    PrimedMod {
        name: "Primed Cleanse The Murmur",
        slug: "primed_cleanse_the_murmur",
    },
    PrimedMod {
        name: "Primed Combustion Rounds",
        slug: "primed_combustion_rounds",
    },
    PrimedMod {
        name: "Primed Continuity",
        slug: "primed_continuity",
    },
    PrimedMod {
        name: "Primed Convulsion",
        slug: "primed_convulsion",
    },
    PrimedMod {
        name: "Primed Counterbalance",
        slug: "primed_counterbalance",
    },
    PrimedMod {
        name: "Primed Cryo Rounds",
        slug: "primed_cryo_rounds",
    },
    PrimedMod {
        name: "Primed Deadly Efficiency",
        slug: "primed_deadly_efficiency",
    },
    PrimedMod {
        name: "Primed Dual Rounds",
        slug: "primed_dual_rounds",
    },
    PrimedMod {
        name: "Primed Expel Corpus",
        slug: "primed_expel_corpus",
    },
    PrimedMod {
        name: "Primed Expel Grineer",
        slug: "primed_expel_grineer",
    },
    PrimedMod {
        name: "Primed Expel Infested",
        slug: "primed_expel_infested",
    },
    PrimedMod {
        name: "Primed Expel Orokin ",
        slug: "primed_expel_corrupted",
    },
    PrimedMod {
        name: "Primed Expel The Murmur",
        slug: "primed_expel_the_murmur",
    },
    PrimedMod {
        name: "Primed Fast Hands",
        slug: "primed_fast_hands",
    },
    PrimedMod {
        name: "Primed Fever Strike",
        slug: "primed_fever_strike",
    },
    PrimedMod {
        name: "Primed Firestorm",
        slug: "primed_firestorm",
    },
    PrimedMod {
        name: "Primed Flow",
        slug: "primed_flow",
    },
    PrimedMod {
        name: "Primed Fulmination",
        slug: "primed_fulmination",
    },
    PrimedMod {
        name: "Primed Heated Charge",
        slug: "primed_heated_charge",
    },
    PrimedMod {
        name: "Primed Heavy Trauma",
        slug: "primed_heavy_trauma",
    },
    PrimedMod {
        name: "Primed Magazine Warp",
        slug: "primed_magazine_warp",
    },
    PrimedMod {
        name: "Primed Morphic Transformer",
        slug: "primed_morphic_transformer",
    },
    PrimedMod {
        name: "Primed Pack Leader",
        slug: "primed_pack_leader",
    },
    PrimedMod {
        name: "Primed Pistol Ammo Mutation",
        slug: "primed_pistol_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Pistol Gambit",
        slug: "primed_pistol_gambit",
    },
    PrimedMod {
        name: "Primed Point Blank",
        slug: "primed_point_blank",
    },
    PrimedMod {
        name: "Primed Pressure Point",
        slug: "primed_pressure_point",
    },
    PrimedMod {
        name: "Primed Quickdraw",
        slug: "primed_quickdraw",
    },
    PrimedMod {
        name: "Primed Ravage",
        slug: "primed_ravage",
    },
    PrimedMod {
        name: "Primed Reach",
        slug: "primed_reach",
    },
    PrimedMod {
        name: "Primed Redirection",
        slug: "primed_redirection",
    },
    PrimedMod {
        name: "Primed Regen",
        slug: "primed_regen",
    },
    PrimedMod {
        name: "Primed Rifle Ammo Mutation",
        slug: "primed_rifle_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Rubedo-Lined Barrel ",
        slug: "primed_rubedo_lined_barrel",
    },
    PrimedMod {
        name: "Primed Shotgun Ammo Mutation",
        slug: "primed_shotgun_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Slip Magazine",
        slug: "primed_slip_magazine",
    },
    PrimedMod {
        name: "Primed Smite Corpus",
        slug: "primed_smite_corpus",
    },
    PrimedMod {
        name: "Primed Smite Grineer",
        slug: "primed_smite_grineer",
    },
    PrimedMod {
        name: "Primed Smite Infested",
        slug: "primed_smite_infested",
    },
    PrimedMod {
        name: "Primed Smite Orokin ",
        slug: "primed_smite_corrupted",
    },
    PrimedMod {
        name: "Primed Smite The Murmur",
        slug: "primed_smite_the_murmur",
    },
    PrimedMod {
        name: "Primed Sniper Ammo Mutation",
        slug: "primed_sniper_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Stabilizer",
        slug: "primed_stabilizer",
    },
    PrimedMod {
        name: "Primed Steady Hands",
        slug: "primed_steady_hands",
    },
    PrimedMod {
        name: "Primed Tactical Pump",
        slug: "primed_tactical_pump",
    },
    PrimedMod {
        name: "Primed Target Cracker",
        slug: "primed_target_cracker",
    },
    PrimedMod {
        name: "Primed Venomous Clip",
        slug: "primed_venomous_clip",
    },
    PrimedMod {
        name: "Astral Twilight",
        slug: "astral_twilight",
    },
    PrimedMod {
        name: "Buzz Kill",
        slug: "buzz_kill",
    },
    PrimedMod {
        name: "Collision Force",
        slug: "collision_force",
    },
    PrimedMod {
        name: "Combo Fury",
        slug: "combo_fury",
    },
    PrimedMod {
        name: "Combo Killer",
        slug: "combo_killer",
    },
    PrimedMod {
        name: "Crash Course",
        slug: "crash_course",
    },
    PrimedMod {
        name: "Fanged Fusillade",
        slug: "fanged_fusillade",
    },
    PrimedMod {
        name: "Full Contact",
        slug: "full_contact",
    },
    PrimedMod {
        name: "High Voltage",
        slug: "high_voltage",
    },
    PrimedMod {
        name: "Jolt",
        slug: "jolt",
    },
    PrimedMod {
        name: "Maim",
        slug: "maim",
    },
    PrimedMod {
        name: "Mark of the Beast",
        slug: "mark_of_the_beast",
    },
    PrimedMod {
        name: "Pummel",
        slug: "pummel",
    },
    PrimedMod {
        name: "Scattering Inferno",
        slug: "scattering_inferno",
    },
    PrimedMod {
        name: "Scorch",
        slug: "scorch",
    },
    PrimedMod {
        name: "Shell Shock",
        slug: "shell_shock",
    },
    PrimedMod {
        name: "Split Flights",
        slug: "split_flights",
    },
    PrimedMod {
        name: "Sweeping Serration",
        slug: "sweeping_serration",
    },
    PrimedMod {
        name: "Tempo Royale",
        slug: "tempo_royale",
    },
    PrimedMod {
        name: "Thermite Rounds",
        slug: "thermite_rounds",
    },
    PrimedMod {
        name: "Vermillion Storm",
        slug: "vermillion_storm",
    },
    PrimedMod {
        name: "Volcanic Edge",
        slug: "volcanic_edge",
    },
    PrimedMod {
        name: "Voltaic Strike",
        slug: "voltaic_strike",
    },
    PrimedMod {
        name: "Peculiar Audience",
        slug: "peculiar_audience",
    },
];

fn get_auto_keep(
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
fn resolve_keep_copies(manual_keep: u32, auto_keep: u32) -> u32 {
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

fn find_same_price_order<'a>(
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

fn resolve_action_choice(raw_input: &str) -> String {
    let trimmed = raw_input.trim().to_uppercase();
    if trimmed.is_empty() {
        "Y".to_string()
    } else {
        trimmed
    }
}

fn decide_no_op(
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
        (true, false) => NoOpDecision::QuantitySyncOnly { new_quantity: desired_total_qty, keep_price: existing_price },
        (false, _) => NoOpDecision::NeedsReview,
    }
}

fn quantity_default(is_already_listed: bool, listed_qty: u32, available_qty: u32) -> u32 {
    if is_already_listed { listed_qty + available_qty } else { available_qty }
}

fn ayatan_max_stars(slug: &str) -> (u8, u8) {
    match slug {
        "ayatan_anasa_sculpture"    => (2, 2),
        "ayatan_ayr_sculpture"      => (3, 0),
        "ayatan_chattraka_sculpture"
        | "ayatan_hemakara_sculpture"
        | "ayatan_piv_sculpture"
        | "ayatan_sah_sculpture"
        | "ayatan_valana_sculpture"
        | "ayatan_vaya_sculpture"
        | "ayatan_zambuka_sculpture" => (2, 1),
        "ayatan_kitha_sculpture"    => (4, 1),
        "ayatan_orta_sculpture"     => (3, 1),
        _                           => (0, 0),
    }
}

pub(crate) fn print_header(title: &str) {
    tsprintln!("\x1B[1;36m================================================================================\x1B[0m");
    tsprintln!("\x1B[1;35m   {}   \x1B[0m", title.to_uppercase());
    tsprintln!("\x1B[1;36m================================================================================\x1B[0m");
}

pub(crate) fn print_info(label: &str, value: &str) {
    tsprintln!("\x1B[1;34m  {label:<25}\x1B[0m : \x1B[32m{value}\x1B[0m");
}

fn print_warning(msg: &str) {
    tsprintln!("\x1B[1;33m  [WARNING] {msg}\x1B[0m");
}

#[allow(dead_code)]
fn print_error_ui(msg: &str) {
    tsprintln!("\x1B[1;31m  [ERROR] {msg}\x1B[0m");
}

// ── Helper functions for `run_cli` ──────────────────────────────────────────

fn load_credentials() -> Result<(String, String), Box<dyn Error + Send + Sync>> {
    let email = std::env::var("WFM_EMAIL").unwrap_or_default();
    let password = std::env::var("WFM_PASSWORD").unwrap_or_default();
    if email.is_empty() || password.is_empty() {
        print_warning("WFM_EMAIL or WFM_PASSWORD not found in environment.");
        print_info("Please supply them", "e.g., set WFM_EMAIL=email in environment or .env file.");
        return Err("Missing credentials".into());
    }
    Ok((email, password))
}

async fn fetch_user_listings(wfm_client: &WfmClient) -> Result<(Vec<OwnedOrder>, HashMap<ListingKey, Vec<OwnedOrder>>), Box<dyn Error + Send + Sync>> {
    tsprintln!("Fetching your active listings from Warframe.Market...");
    let all_orders = wfm_client.my_orders().await?;
    let user_listings: Vec<OwnedOrder> = all_orders.into_iter().filter(OwnedOrder::is_sell).collect();
    let current_count = user_listings.len();
    print_info("Active Listings on WFM", &format!("{current_count}/100 slots used"));

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
fn aggregate_sets_with_prices(
    candidates: Vec<MappedItem>,
    parent_map: &BuildParentMap,
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    prices: &HashMap<String, f64>,
) -> Vec<MappedItem> {
    // Separate parts that belong to a build from everything else
    let mut part_items = Vec::new();
    let mut other_items = Vec::new();

    for item in candidates {
        if parent_map.contains_key(&item.game_ref) {
            part_items.push(item);
        } else {
            other_items.push(item);
        }
    }

    // Build component quantity map: game_ref -> (total_qty, example_item)
    let mut component_qty: HashMap<String, (u32, MappedItem)> = HashMap::new();
    for item in &part_items {
        let key = item.game_ref.clone();
        let (qty, _) = component_qty.entry(key).or_insert((0, item.clone()));
        *qty += item.quantity;
    }

    let mut set_items = Vec::new();
    let mut consumed: HashMap<String, u32> = HashMap::new();

    // Process each build
    for (build_unique, recipe) in requirements {
        let Some(wfcd_item) = wfcd_by_ref.get(build_unique) else { continue };
        let Some(set_item) = resolve_set_item(&wfcd_item.name, wfm_by_name) else { continue };

        // Determine possible sets ignoring guard
        let mut possible_sets = u32::MAX;
        for (comp_unique, required_qty) in recipe {
            if let Some((qty, _)) = component_qty.get(comp_unique) {
                let avail = *qty - consumed.get(comp_unique).copied().unwrap_or(0);
                possible_sets = possible_sets.min(avail / required_qty);
                if possible_sets == 0 {
                    break;
                }
            } else {
                possible_sets = 0;
                break;
            }
        }

        if possible_sets == 0 {
            continue;
        }

        let set_price = *prices.get(&set_item.slug).unwrap_or(&0.0);
        if set_price <= 0.0 {
            continue;
        }

        // Form sets
        let sets_to_form = possible_sets;
        let set_mapped = MappedItem {
            id: set_item.id.clone(),
            slug: set_item.slug.clone(),
            name: set_item.i18n.en.name.clone(),
            quantity: sets_to_form,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: build_unique.clone(),
            subtypes: set_item.subtypes.clone(),
            bulk_tradable: set_item.bulk_tradable,
        };
        set_items.push(set_mapped);

        // Record consumption
        for (comp_unique, required_qty) in recipe {
            *consumed.entry(comp_unique.clone()).or_insert(0) += required_qty * sets_to_form;
        }
    }

    // Build final list: sets + leftovers + other_items
    let mut result = set_items;

    for (comp_unique, (total_qty, comp_item_template)) in component_qty {
        let used = consumed.get(&comp_unique).copied().unwrap_or(0);
        let leftover = total_qty.saturating_sub(used);
        if leftover > 0 {
            // Same "worth a listing slot" heuristic that filter_candidates used to apply
            // pre-aggregation. Applied here — after consumption is accounted for — it only
            // prunes genuinely orphaned components (no completable set exists for them, or
            // they weren't needed for the sets we did form), rather than the raw parts an
            // in-progress set still needs, which must reach aggregation first.
            let name_lower = comp_item_template.name.to_lowercase();
            let worth_reviewing = name_lower.contains("prime") || name_lower.contains("set") || name_lower.contains("blueprint");
            if !worth_reviewing {
                continue;
            }
            let mut leftover_item = comp_item_template.clone();
            leftover_item.quantity = leftover;
            result.push(leftover_item);
        }
    }

    result.extend(other_items);
    result
}

fn filter_candidates(mapped_items: Vec<MappedItem>, parent_map: &BuildParentMap) -> Vec<MappedItem> {
    tsprintln!("Filtering high-value candidates for trade review...");
    mapped_items
        .into_iter()
        .filter(|item| {
            if item.is_arcane || item.is_ayatan {
                return true;
            }
            if item.is_mod {
                return item.max_rank.is_some();
            }
            // Known build components (Barrels, Receivers, Stocks, Chassis, Systems,
            // Neuroptics, Hilts, Blades, Links, Guards, Gauntlets, ...) must always reach
            // aggregate_sets_with_prices intact, even though their display names usually
            // contain none of "prime"/"set"/"blueprint" (only non-Prime part names lack
            // "prime"; only the blueprint itself contains "blueprint"). Dropping them here
            // silently starved the set-aggregator of exactly the pieces it needs to detect a
            // completed set — e.g. a Barrel/Receiver pair with no "prime" or "blueprint" in
            // their names would vanish before aggregation ever saw them, so a genuinely
            // complete set never got formed and its Blueprint sat as a 100% "leftover"
            // instead of being partially consumed. Post-aggregation, true junk components
            // (parts of builds we'll never complete) still get pruned as unconsumed leftovers
            // further down the pipeline via the name filter below — this only protects them
            // from being discarded *before* aggregation gets a chance to use them.
            if parent_map.contains_key(&item.game_ref) {
                return true;
            }
            let name_lower = item.name.to_lowercase();
            name_lower.contains("prime") || name_lower.contains("set") || name_lower.contains("blueprint")
        })
        .collect()
}

/// Computes a mod's upgrade suggestion (price delta, endo cost to max, and a ranking score) if
/// leveling it to `max_rank` would be profitable. Returns `None` if it's already maxed, there's
/// no endo left to spend, or leveling wouldn't raise the price enough to be worth it.
///
/// Pulled out as a pure function specifically so this is unit-testable without a live
/// `build_priced_candidates` pipeline — see `upgrade_suggestion_tests` below, including a
/// regression test for the rank-0 case that was previously, silently, never suggested.
fn upgrade_suggestion(
    rarity: &str,
    current_rank: u32,
    max_rank: u32,
    is_antique: bool,
    wa_price: f64,
    max_price: f64,
    vol_30d: u32,
) -> Option<(f64, u32, f64)> {
    if current_rank >= max_rank {
        return None;
    }
    let endo_cost = get_fusion_cost_from_zero(rarity, current_rank, is_antique);
    let endo_to_max = get_fusion_cost_from_zero(rarity, max_rank, is_antique).saturating_sub(endo_cost);
    if endo_to_max == 0 {
        return None;
    }
    let delta = max_price - wa_price;
    if delta <= 0.0 {
        return None;
    }
    let upgrade_score = (delta / f64::from(endo_to_max)) * (1.0 + f64::from(vol_30d)).ln();
    Some((delta, endo_to_max, upgrade_score))
}

#[cfg(test)]
mod upgrade_suggestion_tests {
    use super::*;

    #[test]
    fn unranked_mod_with_profitable_max_price_is_suggested() {
        // Regression test: get_fusion_cost_from_zero(rank=0) is 0 by definition, which used to
        // be (wrongly) used as a gate for "has this mod been touched at all" — excluding every
        // unranked mod, the single most common case for a sellable duplicate.
        let result = upgrade_suggestion("Rare", 0, 10, false, 10.0, 80.0, 500);
        assert!(result.is_some(), "an unranked mod with a profitable max price must still be suggested");
        let (delta, endo_to_max, _score) = result.unwrap();
        assert!((delta - 70.0).abs() < f64::EPSILON);
        assert!(endo_to_max > 0);
    }

    #[test]
    fn already_maxed_mod_is_not_suggested() {
        assert!(upgrade_suggestion("Rare", 10, 10, false, 10.0, 80.0, 500).is_none());
    }

    #[test]
    fn unprofitable_upgrade_is_not_suggested() {
        // max_price <= wa_price: no point spending endo to "upgrade" into a cheaper or
        // equal price.
        assert!(upgrade_suggestion("Rare", 0, 10, false, 50.0, 40.0, 500).is_none());
    }
}

/// Abstraction over how `build_priced_candidates` retrieves market statistics for a slug.
/// Exists purely so tests can supply fixture data instead of making a real network call —
/// production code always uses `LiveStatsSource`, which just delegates to `fetch_statistics`.
trait StatsSource {
    async fn fetch(&self, slug: &str) -> Result<WfmStatsResponse, Box<dyn Error>>;
}

struct LiveStatsSource;

impl StatsSource for LiveStatsSource {
    async fn fetch(&self, slug: &str) -> Result<WfmStatsResponse, Box<dyn Error>> {
        fetch_statistics(slug).await
    }
}

async fn build_priced_candidates<S: StatsSource>(
    candidates: Vec<MappedItem>,
    _endo_rate: f64, // unused here, needed for signature compatibility
    parent_map: &BuildParentMap,
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    stats_source: &S,
) -> (Vec<(MappedItem, f64, f64, u32, f64)>, Vec<(String, f64, u32, u32, f64)>) {
    // ---- 1. Collect all slugs we need ----
    let mut slugs_to_fetch = HashSet::new();

    // All candidate slugs (including components)
    for item in &candidates {
        slugs_to_fetch.insert(item.slug.clone());
    }

    // Also, for each build, add its set slug (if resolvable)
    for build_unique in requirements.keys() {
        if let Some(wfcd_item) = wfcd_by_ref.get(build_unique)
            && let Some(set_item) = resolve_set_item(&wfcd_item.name, wfm_by_name) {
                slugs_to_fetch.insert(set_item.slug);
            }
    }

    // ---- 2. Fetch stats for all slugs, store in a map ----
    let mut stats_map = HashMap::new();
    for slug in &slugs_to_fetch {
        match stats_source.fetch(slug).await {
            Ok(stats) => {
                stats_map.insert(slug.clone(), stats);
            }
            Err(e) => tseprintln!("Warning: failed to fetch stats for {slug}: {e}"),
        }
    }

    // ---- 3. Build price map (slug -> wa_price) ----
    let mut price_map = HashMap::new();
    for (slug, stats) in &stats_map {
        let (wa_price, _) = calculate_weighted_average(stats, None);
        price_map.insert(slug.clone(), wa_price);
    }

    // ---- 4. Aggregate sets ----
    let aggregated_items = aggregate_sets_with_prices(
        candidates,
        parent_map,
        requirements,
        wfcd_by_ref,
        wfm_by_name,
        &price_map,
    );

    // ---- 5. For each aggregated item, compute pricing details ----
    let mut priced = Vec::new();
    let mut upgrades = Vec::new();

    for item in aggregated_items {
        // Determine the target rank for price calculation (mods/arcanes only)
        let target_rank = if item.is_mod || item.is_arcane { item.rank } else { None };

        // Get stats for this item's slug
        let stats_opt = stats_map.get(&item.slug);
        let (wa_price, _total_vol) = if let Some(stats) = stats_opt {
            calculate_weighted_average(stats, target_rank)
        } else {
            (0.0, 0)
        };

        if wa_price <= 0.0 {
            // Skip items that have no market price
            continue;
        }

        // Recent volume (30 days) — computed once and reused for both the demand-floor check
        // below and the score/display value further down.
        let (vol_30d, _trading_days_30d) = if let Some(stats) = stats_opt {
            recent_volume(stats, target_rank, 30)
        } else {
            (0, 0)
        };

        // For mods, also check volume *at max rank*. An unranked (rank 0) mod is a perfectly
        // good upgrade candidate even if the unranked market itself is thin — what actually
        // matters for "is it worth leveling this up" is whether the *maxed* copy sells, since
        // that's the form you'd list it in after upgrading. Previously the demand floor below
        // used only `vol_30d` (volume at the item's *current* rank), which silently zeroed out
        // upgrade suggestions for every mod sitting at rank 0 with a quiet unranked market —
        // even wildly popular mods, since most owned drops are unranked and unranked copies
        // trade far less than maxed ones.
        let vol_30d_max = if item.is_mod && !item.is_arcane
            && let Some(max_rank) = item.max_rank
            && let Some(stats) = stats_opt {
                recent_volume(stats, Some(max_rank), 30).0
            } else {
                vol_30d
            };

        // ---- Demand floor for mods/arcanes ----
        // Use whichever rank (current or max) has better liquidity — only drop the item if
        // it's illiquid in both forms.
        if item.is_mod || item.is_arcane {
            let vol_per_day = f64::from(vol_30d.max(vol_30d_max)) / 30.0;
            if vol_per_day < MIN_DAILY_VOLUME {
                continue; // below demand floor, skip entirely
            }
        }

        // Saturation ratio
        let saturation = if let Some(stats) = stats_opt {
            calculate_saturation_ratio(stats, target_rank)
        } else {
            0.0
        };

        // Score: price * (1 + ln(volume)) – used for sorting later
        let score = wa_price * (1.0 + f64::from(vol_30d)).ln();

        // ---- Upgrade suggestions (only for mods) ----
        if item.is_mod && !item.is_arcane
            && let Some(max_rank) = item.max_rank
            && let Some(stats) = stats_opt {
                let current_rank_u32 = u32::from(item.rank.unwrap_or(0));
                let max_rank_u32 = u32::from(max_rank);
                let is_antique = is_antique(&item.slug, &item.game_ref);
                let (max_price, _) = calculate_weighted_average(stats, Some(max_rank));
                // Use vol_30d_max here (not vol_30d): the score/volume shown should reflect
                // demand for the mod in the form it'll actually be sold in after upgrading.
                if let Some((delta, endo_to_max, upgrade_score)) = upgrade_suggestion(
                    &item.rarity, current_rank_u32, max_rank_u32, is_antique, wa_price, max_price, vol_30d_max,
                ) {
                    upgrades.push((item.name.clone(), delta, endo_to_max, vol_30d_max, upgrade_score));
                }
            }

        // Store priced candidate
        priced.push((item, wa_price, saturation, vol_30d, score));
    }

    // Sort upgrades by score descending and truncate
    let mut upgrades_sorted = upgrades;
    upgrades_sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    upgrades_sorted.truncate(15);

    (priced, upgrades_sorted)
}

#[cfg(test)]
mod build_priced_candidates_tests {
    use super::*;
    use crate::models::{WfmStatsItem, WfmStatsPayload, WfmStatsSubPayload};

    /// In-memory `StatsSource` for tests — maps slug -> pre-built `WfmStatsResponse`, so a test
    /// never touches the network or the on-disk stats cache that `LiveStatsSource` uses.
    struct FixtureStatsSource(HashMap<String, WfmStatsResponse>);

    impl StatsSource for FixtureStatsSource {
        async fn fetch(&self, slug: &str) -> Result<WfmStatsResponse, Box<dyn Error>> {
            self.0
                .get(slug)
                .cloned()
                .ok_or_else(|| format!("no fixture stats for slug '{slug}'").into())
        }
    }

    /// Builds one day's worth of stats at a given mod rank — enough for `recent_volume`,
    /// `calculate_weighted_average`, and `calculate_saturation_ratio` to all see a single,
    /// recent, unambiguous data point. `order_type: "sell"` matters here specifically because
    /// `calculate_saturation_ratio` filters the live sub-payload on `order_type == "sell"`, and
    /// this fixture reuses the same item list for both the closed and live sub-payloads.
    fn stats_item(mod_rank: u32, wa_price: f64, volume: u32) -> WfmStatsItem {
        WfmStatsItem {
            datetime: "2026-06-20T00:00:00.000Z".to_string(),
            volume,
            min_price: wa_price,
            max_price: wa_price,
            avg_price: Some(wa_price),
            wa_price,
            median: wa_price,
            moving_avg: None,
            rank: Some(mod_rank),
            order_type: Some("sell".to_string()),
        }
    }

    fn stats_response(items: Vec<WfmStatsItem>) -> WfmStatsResponse {
        WfmStatsResponse {
            payload: WfmStatsPayload {
                statistics_closed: WfmStatsSubPayload { ninety_days: items.clone() },
                statistics_live: WfmStatsSubPayload { ninety_days: items },
            },
        }
    }

    fn unranked_mod_candidate(slug: &str, max_rank: u8) -> MappedItem {
        MappedItem {
            id: format!("{slug}_id"),
            slug: slug.to_string(),
            name: "Test Mod".to_string(),
            quantity: 3,
            rank: Some(0),
            max_rank: Some(max_rank),
            rarity: "Rare".to_string(),
            is_mod: true,
            is_arcane: false,
            is_ayatan: false,
            game_ref: "/Lotus/Upgrades/Mods/TestMod".to_string(),
            subtypes: vec![],
            bulk_tradable: false,
        }
    }

    #[tokio::test]
    async fn unranked_mod_surfaces_an_upgrade_suggestion_end_to_end() {
        // Regression test for the bug where get_fusion_cost_from_zero(rank=0) == 0 was used as
        // a gate, silently excluding every unranked mod — the most common real candidate — from
        // ever reaching the upgrade list. This exercises the full build_priced_candidates
        // pipeline (slug collection, stats fetch via the seam, demand floor, scoring, and the
        // upgrade-suggestion block) rather than just the pure upgrade_suggestion() function.
        let candidate = unranked_mod_candidate("primed_pressure_point", 10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "primed_pressure_point".to_string(),
            stats_response(vec![
                stats_item(0, 10.0, 500),   // current (unranked) price/volume
                stats_item(10, 80.0, 500),  // max-rank price
            ]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (priced, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0, // endo_rate, unused by this path
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert_eq!(priced.len(), 1, "the candidate should still be priced");
        assert!(!upgrades.is_empty(), "an unranked mod with a profitable max price must surface an upgrade suggestion");
        assert_eq!(upgrades[0].0, "Test Mod");
    }

    #[tokio::test]
    async fn already_maxed_mod_does_not_surface_an_upgrade_suggestion() {
        let mut candidate = unranked_mod_candidate("maxed_mod", 10);
        candidate.rank = Some(10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "maxed_mod".to_string(),
            stats_response(vec![stats_item(10, 80.0, 500)]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (_, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0,
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert!(upgrades.is_empty(), "an already-maxed mod has nothing left to upgrade into");
    }

    #[tokio::test]
    async fn low_volume_mod_is_filtered_before_reaching_the_upgrade_check() {
        // Below config::MIN_DAILY_VOLUME — should be skipped by the demand floor before
        // the upgrade-suggestion block ever runs, even though it would otherwise qualify.
        let candidate = unranked_mod_candidate("illiquid_mod", 10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "illiquid_mod".to_string(),
            stats_response(vec![
                stats_item(0, 10.0, 1),  // 1 sale in 30 days, well under the 9.0/day floor
                stats_item(10, 80.0, 1),
            ]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (priced, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0,
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert!(priced.is_empty(), "below the demand floor, the candidate should be dropped entirely");
        assert!(upgrades.is_empty());
    }

    #[tokio::test]
    async fn quiet_unranked_but_liquid_maxed_mod_still_surfaces_an_upgrade() {
        // Regression test: most owned mod copies sit at rank 0 (fresh drops), and the
        // *unranked* market for a mod is routinely much quieter than its maxed market even
        // for mods that are extremely popular once leveled (e.g. Serration-tier mods — nobody
        // farms endo to buy an unranked one, everyone wants it maxed). Gating the demand floor
        // (and the upgrade score) on current-rank volume alone silently dropped exactly this
        // case. Volume at rank 0 is below the floor; volume at max rank is well above it — the
        // item must still be priced and must still surface an upgrade suggestion.
        let candidate = unranked_mod_candidate("popular_when_maxed", 10);

        let mut fixtures = HashMap::new();
        fixtures.insert(
            "popular_when_maxed".to_string(),
            stats_response(vec![
                stats_item(0, 10.0, 2),     // unranked: 2 sales/30d, well under the floor
                stats_item(10, 80.0, 900),  // maxed: 900 sales/30d, comfortably liquid
            ]),
        );
        let stats_source = FixtureStatsSource(fixtures);

        let (priced, upgrades) = build_priced_candidates(
            vec![candidate],
            0.0,
            &BuildParentMap::new(),
            &BuildRequirements::new(),
            &HashMap::new(),
            &HashMap::new(),
            &stats_source,
        )
        .await;

        assert_eq!(priced.len(), 1, "liquid-when-maxed candidate should still be priced");
        assert!(!upgrades.is_empty(), "should still surface an upgrade suggestion despite thin unranked volume");
        assert_eq!(upgrades[0].0, "Test Mod");
    }
}

fn print_upgrade_suggestions(suggestions: &[(String, f64, u32, u32, f64)]) {
    let mut sorted = suggestions.to_vec();
    sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(15);

    print_header("Mod Upgrade Suggestions (Best Endo Value × Volume)");
    tsprintln!("\x1B[1m  {:<35} | {:<14} | {:<12} | {:<10} | Score\x1B[0m",
        "Mod", "Δ Plat (→max)", "Endo Cost", "30d Vol");
    tsprintln!("  {}", "-".repeat(82));
    for (name, delta, endo, vol, score) in &sorted {
        tsprintln!("  {name:<35} | {delta:<14.1} | {endo:<12} | {vol:<10} | {score:.4}");
    }
    tsprintln!();
}

fn sort_candidates(
    mut priced: Vec<(MappedItem, f64, f64, u32, f64)>,
    existing: &HashMap<ListingKey, Vec<OwnedOrder>>,
) -> Vec<(MappedItem, f64, f64, u32, f64)> {
    priced.sort_by(|a, b| {
        let a_key = ListingKey { item_id: a.0.id.clone(), rank: if a.0.is_mod || a.0.is_arcane { a.0.rank } else { None } };
        let b_key = ListingKey { item_id: b.0.id.clone(), rank: if b.0.is_mod || b.0.is_arcane { b.0.rank } else { None } };
        let a_listed = existing.contains_key(&a_key);
        let b_listed = existing.contains_key(&b_key);
        if a_listed && !b_listed {
            std::cmp::Ordering::Less
        } else if !a_listed && b_listed {
            std::cmp::Ordering::Greater
        } else {
            b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    priced
}

// This function is a single linear decision pipeline (keep/blacklist checks, price-vs-Endo
// comparison, listing-state lookup, no-op/quantity-sync short-circuit, then the interactive
// prompt). Splitting it purely to satisfy the line-count lint would mean threading the same
// half-dozen pieces of state through several smaller functions for no behavioral benefit;
// left as-is deliberately rather than fixed blindly without a compiler to verify a refactor.
#[allow(clippy::too_many_lines)]
async fn handle_single_candidate(
    mut item: MappedItem,
    wa_price: f64,
    saturation: f64,
    vol_30d: u32,
    ctx: &mut CandidateContext<'_>,
) -> Result<Option<SessionReportItem>, Box<dyn Error + Send + Sync>> {
    // ── Keep list / blacklist handling ─────────────────────────────────────
    if ctx.blacklist_set.slugs.contains(&item.slug) {
        return Ok(None);
    }

    // Mods/arcanes: keep-reservation already happened once, cross-rank, in
    // `mapping::apply_cross_rank_keep`. Re-running per-rank here would double-reserve against
    // quantities that are already net of the keep. Everything else still resolves it here.
    let manual_keep = if item.is_mod || item.is_arcane {
        0
    } else {
        get_keep_quantity(ctx.keeplist, &item.slug, item.rank, item.category())
    };
    let auto_keep = get_auto_keep(&item, ctx.parent_map, ctx.mastered_set, ctx.owned_built_set);
    let keep_copies = resolve_keep_copies(manual_keep, auto_keep);
    if keep_copies > 0 {
        if item.quantity <= keep_copies { return Ok(None); }
        item.quantity -= keep_copies;
    }

    if item.is_ayatan && let Some(endo_yield) = get_ayatan_endo_yield(&item.slug) {
        let endo_value = f64::from(endo_yield) * ctx.endo_rate;
        if wa_price < endo_value * 1.15 {
            tsprintln!("[SKIP] {} worth {:.1}p as Endo (vs {:.1}p market)", item.name, endo_value, wa_price);
            return Ok(None);
        }
    }

    let listing_key = ListingKey {
        item_id: item.id.clone(),
        rank: if item.is_mod || item.is_arcane { item.rank } else { None },
    };

    let matching_listings = ctx.existing_listings_map.get(&listing_key);
    let listed_qty: u32 = matching_listings.map_or(0, |listings| {
        listings.iter()
          .map(|l| l.quantity())
          .sum()
    });

    let available_qty = item.quantity.saturating_sub(listed_qty);
    let is_already_listed = matching_listings.is_some();

    if available_qty == 0 { return Ok(None); }

    if *ctx.active_slots_count >= 100 && !is_already_listed {
        print_warning(&format!("Budget limit reached (100/100 slots). Skipping listing creation candidate: {}", item.name));
        return Ok(None);
    }

    // ── Silent no‑op / quantity‑sync for already‑listed items ──────────────
    if is_already_listed {
        // wa_price is always a non-negative market price, well within u32 range.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let suggested_price = wa_price.round() as u32;
        let desired_total_qty = listed_qty + available_qty;

        // Get the first existing listing for this key
        if let Some(listings) = ctx.existing_listings_map.get(&listing_key)
            && let Some(existing) = listings.first()
        {
            let decision = decide_no_op(
                suggested_price,
                existing.platinum(),
                desired_total_qty,
                existing.quantity(),
            );

            match decision {
                NoOpDecision::TrueNoOp => return Ok(None),
                NoOpDecision::QuantitySyncOnly { new_quantity, keep_price } => {
                    // Silently update the listing with the new quantity, keeping the price exactly as-is.
                    let update = UpdateOrder::new().platinum(keep_price).quantity(new_quantity);
                    if let Err(e) = ctx.wfm_client.update_order(existing.id(), update).await {
                        tseprintln!("\x1B[31m[SYNC_ERROR] Failed to sync quantity for {}: {}\x1B[0m", item.name, e);
                        return Ok(None);
                    }
                    // Return a report item so the session report records the sync.
                    return Ok(Some(SessionReportItem {
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        price: keep_price,
                        quantity: new_quantity,
                        rank: item.rank.map(u32::from),
                        action: "Updated (qty sync, no prompt)".to_string(),
                    }));
                }
                NoOpDecision::NeedsReview => {
                    // Fall through to normal prompt flow.
                }
            }
        }
    }

    tsprintln!("\x1B[1;36m--------------------------------------------------------------------------------\x1B[0m");
    tsprintln!("\x1B[1mCANDIDATE\x1B[0m: \x1B[1;32m{}\x1B[0m | Slug: {} | Qty Available: {}", item.name, item.slug, available_qty);
    tsprintln!("  Rank: {:<5} | 30d Vol: {:<6} | Est Price (WA): \x1B[1;33m{:.1} plat\x1B[0m", item.rank.unwrap_or(0), vol_30d, wa_price);
    tsprintln!("  Saturation Ratio: {saturation:.3} (sell volume vs closed volume)");
    tsprintln!("  Already Listed on WFM: {}", if is_already_listed { "\x1B[1;32mYES\x1B[0m" } else { "\x1B[31mNO\x1B[0m" });

    if is_already_listed && let Some(listings) = ctx.existing_listings_map.get(&listing_key) {
        for (idx, listing) in listings.iter().enumerate() {
            tsprintln!("    [{}] Listed price: {} plat | Qty listed: {} | Visible: {}", idx + 1, listing.platinum(), listing.quantity(), listing.is_visible());
        }
    }

    tsprint!("\x1B[1;35m  Action? [Enter/Y] List/Update | [N] Skip | [K] Add to Keep List | [B] Blacklist | [X] Save & Exit: \x1B[0m");
    let _ = ctx.stdout.flush();
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let choice = resolve_action_choice(&choice);

    if choice == "X" {
        return Err("EXIT_REQUESTED".into());
    }

    if choice == "B" {
        tsprintln!("Blacklisting {} permanently...", item.name);
        ctx.blacklist_set.slugs.insert(item.slug.clone());
        save_blacklist(ctx.blacklist_set)?;
        return Ok(None);
    }

    if choice == "K" {
        tsprint!("\x1B[1;34m  How many copies of {} (rank {}) do you want to keep? \x1B[0m", item.name, item.rank.unwrap_or(0));
        let _ = ctx.stdout.flush();
        let mut keep_str = String::new();
        io::stdin().read_line(&mut keep_str)?;
        if let Ok(keep_qty) = keep_str.trim().parse::<u32>() {
            add_to_keeplist(ctx.keeplist, &item.slug, item.rank, keep_qty)?;
            tsprintln!("Saved to keeplist.json!");
        }
        return Ok(None);
    }

    if choice == "Y" {
      return handle_list_or_update(
          &item,
          wa_price,
          available_qty,
          listed_qty,
          is_already_listed,
          &listing_key,
          ctx,
      ).await;
    }

    Ok(None)
}

// Same rationale as handle_single_candidate above: this is a single interactive prompt-then-act
// sequence (price prompt, quantity prompt, price-conflict detection, sculpture/per-trade prompts,
// create-or-update). Deliberately left as one function rather than split blind.
#[allow(clippy::too_many_lines)]
async fn handle_list_or_update(
    item: &MappedItem,
    wa_price: f64,
    available_qty: u32,
    listed_qty: u32,
    is_already_listed: bool,
    listing_key: &ListingKey,
    ctx: &mut CandidateContext<'_>,
) -> Result<Option<SessionReportItem>, Box<dyn Error + Send + Sync>> {
    // Price prompt
    tsprint!("  Price to list (default {wa_price:.1}): ");
    let _ = ctx.stdout.flush();
    let mut price_str = String::new();
    io::stdin().read_line(&mut price_str)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let default_price = wa_price.round() as u32;
    let price: u32 = price_str.trim().parse::<u32>().unwrap_or(default_price);

    // Quantity prompt
    let quantity_default = quantity_default(is_already_listed, listed_qty, available_qty);
    tsprint!("  Quantity to list (default {quantity_default}): ");
    let _ = ctx.stdout.flush();
    let mut qty_str = String::new();
    io::stdin().read_line(&mut qty_str)?;
    let quantity: u32 = qty_str.trim().parse::<u32>().unwrap_or(quantity_default);

    let mut cyan_stars: Option<u8> = None;
    let mut amber_stars: Option<u8> = None;
    let mut per_trade: Option<u32> = None;

    // ── Price‑conflict detection ──────────────────────────────────────────────
    let existing_same_price_order = find_same_price_order(
        ctx.existing_listings_map,
        &item.id,
        listing_key.rank,
        price,
    );

    if let Some(order) = existing_same_price_order {
        tsprintln!("\x1B[33m[SYNC] Found an existing order for {} at the same price ({} plat). Updating its quantity to {}...\x1B[0m",
            item.name, price, quantity);
        let update = UpdateOrder::new().platinum(price).quantity(quantity);
        match ctx.wfm_client.update_order(order.id(), update).await {
            Ok(()) => {
                tsprintln!("\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m", item.name);
                return Ok(Some(SessionReportItem {
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    price,
                    quantity,
                    rank: item.rank.map(u32::from),
                    action: "Updated (price conflict)".to_string(),
                }));
            }
            Err(e) => {
                tseprintln!("\x1B[31m[SYNC_ERROR] Failed to update existing order: {e}\x1B[0m");
                return Ok(None);
            }
        }
    }

    // ── perTrade ──────────────────────────────────────────────────────────────
    // WFM requires `perTrade` on order creation for any bulk-tradable item (this includes
    // ayatan stars/sculptures, but also plain stackable resources like Endo) — omitting it
    // fails with `"perTrade":"app.field.required"`. We always list 1 unit per trade; the
    // `quantity` field is what actually controls how many are offered overall.
    if item.bulk_tradable {
        per_trade = Some(1);
    }

    // ── Ayatan star prompts ───────────────────────────────────────────────────
    if item.is_ayatan {
        if item.slug.ends_with("_sculpture") {
            let (max_cyan, max_amber) = ayatan_max_stars(&item.slug);
            tsprint!("  Cyan Stars installed (default {max_cyan}): ");
            let _ = ctx.stdout.flush();
            let mut c_str = String::new();
            io::stdin().read_line(&mut c_str)?;
            cyan_stars = Some(c_str.trim().parse::<u8>().unwrap_or(max_cyan));

            tsprint!("  Amber Stars installed (default {max_amber}): ");
            let _ = ctx.stdout.flush();
            let mut a_str = String::new();
            io::stdin().read_line(&mut a_str)?;
            amber_stars = Some(a_str.trim().parse::<u8>().unwrap_or(max_amber));
        }
    }

    // ── Handle update or create ──────────────────────────────────────────────
    if is_already_listed {
        if let Some(listings) = ctx.existing_listings_map.get(listing_key)
            && let Some(first_listing) = listings.first()
        {
            tsprintln!("\x1B[33m[SYNC] Updating listing: {} to {} plat...\x1B[0m", item.name, price);
            sleep(Duration::from_millis(400)).await;
            let update = UpdateOrder::new().platinum(price).quantity(quantity);
            match ctx.wfm_client.update_order(first_listing.id(), update).await {
                Ok(()) => {
                    tsprintln!("\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m", item.name);
                    return Ok(Some(SessionReportItem {
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        price,
                        quantity,
                        rank: None,
                        action: "Updated".to_string(),
                    }));
                }
                Err(e) => {
                    tseprintln!("\x1B[31m[SYNC_ERROR] Failed to update listing {}: {}\x1B[0m", first_listing.id(), e);
                    return Ok(None);
                }
            }
        }
    } else {
        let rank_opt = if item.is_mod || item.is_arcane { item.rank } else { None };
        tsprintln!("\x1B[33m[SYNC] Posting listing: {} (rank: {:?}) for {} plat...\x1B[0m", item.name, rank_opt, price);
        sleep(Duration::from_millis(400)).await;

        // ── Build order using item ID ──────────────────────────────────────
        let mut order = CreateOrder::sell(&item.id, price, quantity);
        if let Some(r) = rank_opt {
            order = order.with_mod_rank(r);
        }

        // ── Subtype handling (data‑driven) ──────────────────────────────────
        if !item.subtypes.is_empty() {
            // Default to the first subtype. Uncomment the block below to prompt the user.
            order = order.with_subtype(&item.subtypes[0]);
            /*
            tsprintln!("This item supports subtypes: {:?}", item.subtypes);
            tsprint!("Choose subtype (default {}): ", item.subtypes[0]);
            let _ = ctx.stdout.flush();
            let mut choice = String::new();
            io::stdin().read_line(&mut choice)?;
            let selected = choice.trim();
            let subtype = if selected.is_empty() || !item.subtypes.contains(&selected.to_string()) {
                &item.subtypes[0]
            } else {
                selected
            };
            order = order.with_subtype(subtype);
            */
        }

        // ── Ayatan stars ──────────────────────────────────────────────────────
        if item.is_ayatan {
            let (max_cyan, max_amber) = ayatan_max_stars(&item.slug);
            cyan_stars = Some(cyan_stars.unwrap_or(max_cyan).min(max_cyan));
            amber_stars = Some(amber_stars.unwrap_or(max_amber).min(max_amber));

            if let (Some(c), Some(a)) = (cyan_stars, amber_stars)
                && (c > 0 || a > 0)
            {
                order = order.with_sculpture_stars(a, c);
            }
        }
        if let Some(pt) = per_trade {
            order = order.with_per_trade(pt);
        }

        match ctx.wfm_client.create_order(order).await {
            Ok(()) => {
                tsprintln!("\x1B[32m[SYNC] Successfully listed {} x{}!\x1B[0m", item.name, quantity);
                *ctx.active_slots_count += 1;
                return Ok(Some(SessionReportItem {
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    price,
                    quantity,
                    rank: rank_opt.map(u32::from),
                    action: "Created".to_string(),
                }));
            }
            Err(e) => {
                tseprintln!("\x1B[31m[SYNC_ERROR] Failed to list {}: {}\x1B[0m", item.name, e);
                return Ok(None);
            }
        }
    }

    Ok(None)
}

// This is an internal orchestration function (not part of any public API) that exists purely to
// build the CandidateContext shared by every candidate in the loop below; some of its parameters
// are borrowed with lifetimes tied to state created inside this function (e.g. `stdout`), so the
// context can't simply be constructed by the caller and passed in as one value instead.
#[allow(clippy::too_many_arguments)]
async fn process_candidates(
    priced_candidates: Vec<(MappedItem, f64, f64, u32, f64)>,
    existing_listings_map: &HashMap<ListingKey, Vec<OwnedOrder>>,
    wfm_client: &WfmClient,
    endo_rate: f64,
    blacklist_set: &mut BlacklistConfig,
    keeplist: &mut KeepConfig,
    active_slots_count: &mut usize,
    parent_map: &BuildParentMap,
    mastered_set: &HashSet<String>,
    owned_built_set: &HashSet<String>,
) -> Result<Vec<SessionReportItem>, Box<dyn Error + Send + Sync>> {
    let mut session_items = Vec::new();
    let mut stdout = io::stdout();
    let mut ctx = CandidateContext {
        existing_listings_map,
        wfm_client,
        endo_rate,
        blacklist_set,
        keeplist,
        active_slots_count,
        stdout: &mut stdout,
        parent_map,
        mastered_set,
        owned_built_set,
    };

    for (item, wa_price, saturation, vol_30d, _score) in priced_candidates {
        match handle_single_candidate(
            item,
            wa_price,
            saturation,
            vol_30d,
            &mut ctx,
        ).await {
            Ok(Some(report_item)) => session_items.push(report_item),
            Ok(None) => {},
            Err(e) if e.to_string() == "EXIT_REQUESTED" => break,
            Err(e) => return Err(e),
        }
    }

    Ok(session_items)
}

fn write_session_report(report: &SessionReport) -> Result<(), Box<dyn Error + Send + Sync>> {
    let content = serde_json::to_string_pretty(report)?;
    fs::write("session_report.json", content)?;
    Ok(())
}

// ── Main CLI entry ──────────────────────────────────────────────────────────

/// Runs the interactive CLI loop.
///
/// # Errors
/// Returns an error if:
/// - Credentials are missing from environment.
/// - WFM client authentication fails.
/// - Network or file I/O operations fail.
/// - TOML or JSON serialization/deserialization fails.
pub async fn run_cli(
    mapped_items: Vec<MappedItem>,
    parent_map: &BuildParentMap,
    mastered_set: &HashSet<String>,
    owned_built_set: &HashSet<String>,
    requirements: &BuildRequirements,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    print_header("Warframe.Market Advisor Session Init");

    let (email, password) = load_credentials()?;
    let creds = Credentials::new(&email, &password, Credentials::generate_device_id());
    let wfm_client = WfmClient::from_credentials(creds).await?;
    let username = wfm_client.get_username().await?;

    let (user_listings, existing_listings_map) = fetch_user_listings(&wfm_client).await?;
    let candidates = filter_candidates(mapped_items, parent_map);
    tsprintln!("Identified {} tradeable high-value candidates.", candidates.len());

    tsprintln!("Deriving dynamic Endo exchange rate from Ayatan prices...");
    let endo_rate = derive_endo_to_plat_from_mods().await;
    print_info("Derived Endo Rate", &format!("{:.5} plat/endo (or {:.1} plat per 1000 endo)", endo_rate, endo_rate * 1000.0));

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
        ).await;
    print_upgrade_suggestions(&upgrade_suggestions);

    let priced_candidates = sort_candidates(priced_candidates, &existing_listings_map);

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
    ).await?;

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

/// A priced (or attempted-to-price) incomplete Set: what buying the missing parts off the
/// current buy-order book would cost, what the completed Set currently fetches on its own
/// sell/ask book, and the resulting profit — or, if pricing couldn't complete, why not.
struct PricedIncompleteSet {
    name: String,
    missing: Vec<(String, u32, u32, bool)>, // (part name, deficit qty, unit price, priced off the ask?)
    total_cost: u32,
    set_sell_price: u32,
    profit: f64,
}

/// `--check-sets`: finds every Set the user owns at least one component of but hasn't
/// finished assembling, then checks whether buying the missing parts is profitable:
///
/// - Cost side: the current best *buy*-order price for each missing part × how many are
///   needed. This is what you'd realistically pay by placing a competitive buy order of your
///   own, not the (higher) instant-buy ask. If a part has no buy orders at all, falls back to
///   its current best *sell*-order price instead of giving up on that Set — flagged with a
///   `*` in the output since it's a worse-case estimate (paying the ask, not getting filled at
///   your own bid).
/// - Revenue side: the current best *sell*-order price for the completed Set — what you'd
///   realistically get by listing/undercutting into the existing ask, not a statistics-derived
///   `wa_price` and not the (lower) buy-order/bid side.
///
/// profit = set sell price − total missing-part cost.
///
/// Every incomplete Set found is reported up front (name + missing parts), before any pricing
/// happens, so the completeness detection itself — `mapping::find_incomplete_sets`, which is
/// driven entirely by "does this build have a sellable WFM Set listing", not by "can this be
/// crafted from a blueprint" — can be sanity-checked independently of pricing. Sets that can't
/// be fully priced (no current orders on one side or the other) are still shown in the final
/// table, just with `N/A` in place of a number and a reason, rather than disappearing.
///
/// Does not place any orders — this is a read-only profitability report. Sunk cost of the
/// parts you already own is intentionally not counted against the profit figure.
///
/// # Errors
/// Returns an error if caches can't be refreshed/loaded, the inventory can't be ingested, or
/// inventory-to-WFM mapping fails.
pub async fn run_check_sets_cli(min_profit: Option<f64>) -> Result<(), Box<dyn Error>> {
    print_header("Incomplete Set Profitability Check");

    mapping::update_caches().await?;

    tsprintln!("Ingesting inventory...");
    let inventory_path = crate::resolve_inventory_path(None)?;
    let inventory = crate::ingestion::ingest_inventory(&inventory_path)?;

    let client = reqwest::Client::new();
    let mapped_items = mapping::map_inventory(&inventory, &client).await?;
    let (_parent_map, requirements) = mapping::load_build_maps()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, _wfm_by_slug) = mapping::load_lookup_tables()?;

    let incomplete = mapping::find_incomplete_sets(&mapped_items, &requirements, &wfcd_by_ref, &wfm_by_ref, &wfm_by_name);
    if incomplete.is_empty() {
        tsprintln!("No incomplete Sets found — every Set you own parts of is either complete already or you don't own any of its parts yet.");
        return Ok(());
    }

    // Report everything the completeness check found, unconditionally, before touching the
    // network. This is the audit trail for `find_incomplete_sets` itself — if something looks
    // wrong here (a Set that shouldn't be sellable, a missing-part quantity that looks off,
    // etc.), that's a detection-logic bug, independent of anything the pricing pass below does.
    print_header(&format!("Incomplete Sets Found ({})", incomplete.len()));
    for set in &incomplete {
        let name = wfcd_by_ref.get(&set.build_unique).map_or(set.build_unique.as_str(), |w| w.name.as_str());
        let parts: Vec<String> = set.missing.iter().map(|c| format!("{}x {}", c.deficit, c.name)).collect();
        tsprintln!("  {name}: needs {}", parts.join(", "));
    }
    tsprintln!("\nChecking current buy/sell orders for each (respects WFM's 3 req/s limit, so this may take a bit)...\n");

    let mut priced = Vec::new();

    for set in &incomplete {
        let Some(wfcd_item) = wfcd_by_ref.get(&set.build_unique) else { continue };
        // find_incomplete_sets already only returns builds with a resolvable WFM Set
        // listing, so this should always succeed — treated as a hard skip (not silently
        // dropped: it still shows up in the "Incomplete Sets Found" list above) if the two
        // ever disagree.
        let Some(set_wfm_item) = resolve_set_item(&wfcd_item.name, &wfm_by_name) else {
            tseprintln!("[WARNING] '{}' passed completeness detection but has no resolvable WFM Set listing — this is a bug, please report it.", wfcd_item.name);
            continue;
        };

        let mut unpriced_reason: Option<String> = None;

        let set_sell_price = match wfm_client::fetch_item_orders(&client, &set_wfm_item.slug).await {
            Ok(orders) => wfm_client::best_sell_price(&orders, None),
            Err(e) => {
                unpriced_reason = Some(format!("failed to fetch orders for the Set: {e}"));
                None
            }
        };
        sleep(Duration::from_millis(350)).await;

        if unpriced_reason.is_none() && set_sell_price.is_none() {
            unpriced_reason = Some("no current sell orders for the completed Set".to_string());
        }

        let mut total_cost: u32 = 0;
        let mut priced_missing = Vec::new();

        for comp in &set.missing {
            if unpriced_reason.is_some() {
                break;
            }

            let Some(wfm_comp) = wfm_by_ref.get(&comp.unique_name) else {
                unpriced_reason = Some(format!("'{}' isn't in the WFM items cache", comp.name));
                break;
            };

            let orders = match wfm_client::fetch_item_orders(&client, &wfm_comp.slug).await {
                Ok(o) => o,
                Err(e) => {
                    unpriced_reason = Some(format!("failed to fetch orders for '{}': {e}", comp.name));
                    break;
                }
            };
            sleep(Duration::from_millis(350)).await;

            let (unit_price, used_ask) = match wfm_client::best_buy_price(&orders, None) {
                Some(price) => (price, false),
                // No buy orders to competitively bid against — fall back to the current
                // best sell/ask price instead of giving up on pricing this Set entirely.
                // Worse-case cost estimate (you're paying the ask instead of getting
                // filled at your own bid), so it's flagged with a `*` in the output.
                None => match wfm_client::best_sell_price(&orders, None) {
                    Some(price) => (price, true),
                    None => {
                        unpriced_reason = Some(format!("no current buy or sell orders for missing part '{}'", comp.name));
                        break;
                    }
                },
            };

            total_cost += unit_price * comp.deficit;
            priced_missing.push((comp.name.clone(), comp.deficit, unit_price, used_ask));
        }

        if let Some(reason) = unpriced_reason {
            tsprintln!("{}: could not price — {reason}.", wfcd_item.name);
            continue;
        }

        // Reachable only once set_sell_price is confirmed Some above.
        let set_sell_price = set_sell_price.unwrap_or(0);
        let profit = f64::from(set_sell_price) - f64::from(total_cost);
        priced.push(PricedIncompleteSet {
            name: wfcd_item.name.clone(),
            missing: priced_missing,
            total_cost,
            set_sell_price,
            profit,
        });
    }

    priced.sort_by(|a, b| b.profit.partial_cmp(&a.profit).unwrap_or(std::cmp::Ordering::Equal));

    print_header("Set Completion Profitability");
    tsprintln!("{:<32} {:>12} {:>15} {:>10}", "Set", "Parts Cost", "Set Sell Price", "Profit");
    tsprintln!("{}", "-".repeat(73));

    let mut shown = 0;
    for set in &priced {
        if let Some(min) = min_profit
            && set.profit < min {
                continue;
            }
        shown += 1;
        tsprintln!(
            "{:<32} {:>12} {:>15} {:>10.1}",
            set.name,
            set.total_cost,
            set.set_sell_price,
            set.profit
        );
        for (part_name, deficit, unit_price, used_ask) in &set.missing {
            if *used_ask {
                tsprintln!("    need {deficit}x {part_name} @ {unit_price}p* (no buy orders — priced off current best sell order instead)");
            } else {
                tsprintln!("    need {deficit}x {part_name} @ {unit_price}p (current best buy order)");
            }
        }
    }

    if shown == 0 {
        tsprintln!("(No priced Sets met the requested minimum profit.)");
    }

    Ok(())
}

pub async fn run_primed_mod_prices(min_rank: bool) -> Result<(), Box<dyn Error>> {
    print_header(if min_rank { "Primed Mod Prices (Unranked)" } else { "Primed Mod Prices (Maxed)" });

    tsprintln!("Fetching current market statistics...\n");

    // Max rank varies per mod (most Primed set mods cap at 5, but some — e.g. the
    // ammo-mutation/ammo-chain/ammo-stock mods — cap lower or higher). We used to
    // hardcode Some(10) here, which silently returned (0.0, 0) for every mod whose
    // real max rank wasn't exactly 10, since calculate_weighted_average/recent_volume
    // filter WFM stats on an exact rank match. Pull the real maxRank from the WFM
    // items cache (keyed by slug) instead, so this can't drift out of sync again.
    // Still needed even in --min-rank mode, just to confirm each slug is a known item.
    let (_wfcd_by_ref, _wfm_by_ref, _wfm_by_name, wfm_by_slug) =
        crate::mapping::load_lookup_tables()?;

    let mut prices = Vec::<PrimedPrice>::new();

    for primed in PRIMED_MODS {
        let Some(raw_max_rank) = wfm_by_slug.get(primed.slug).and_then(|item| item.max_rank) else {
            tseprintln!(
                "[WARNING] Could not resolve max rank for {} ('{}') from WFM items cache — skipping.",
                primed.name,
                primed.slug
            );
            continue;
        };
        // WFM's statistics rank field is u32 while max_rank on WfmItem is also u32,
        // but calculate_weighted_average/recent_volume take Option<u8> — narrow safely.
        let Ok(raw_max_rank) = u8::try_from(raw_max_rank) else {
            tseprintln!(
                "[WARNING] Max rank {} for {} doesn't fit in u8 — skipping.",
                raw_max_rank,
                primed.name
            );
            continue;
        };
        // Unranked is always rank 0 regardless of the mod's max rank; --min-rank just
        // pins the target to that instead of raw_max_rank.
        let target_rank: u8 = if min_rank { 0 } else { raw_max_rank };

        // calculate_weighted_average/recent_volume now self-correct if this guessed rank
        // doesn't match any real statistics row but null-ranked rows exist instead (see
        // resolve_target_rank in pricing.rs) — e.g. items like "Peculiar Audience" that WFM
        // tracks with rank: null on every row rather than numeric ranks.
        match fetch_statistics(primed.slug).await {
            Ok(stats) => {
                let (price, _) = calculate_weighted_average(&stats, Some(target_rank));
                let (volume, _) = recent_volume(&stats, Some(target_rank), 30);

                prices.push(PrimedPrice {
                    name: primed.name,
                    price,
                    volume,
                });
            }

            Err(err) => {
                tseprintln!(
                    "[WARNING] Failed to fetch {}: {}",
                    primed.name,
                    err
                );
            }
        }
    }

    prices.sort_by(|a, b| {
        b.price
            .partial_cmp(&a.price)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    tsprintln!(
        "{:<4} {:<34} {:>10} {:>10}",
        "#",
        "Mod",
        "Price",
        "30d Vol"
    );

    tsprintln!("{}", "-".repeat(64));

    for (i, mod_price) in prices.iter().enumerate() {
        tsprintln!(
            "{:<4} {:<34} {:>10.1} {:>10}",
            i + 1,
            mod_price.name,
            mod_price.price,
            mod_price.volume,
        );
    }

    Ok(())
}

// ── Blacklist / Keeplist helpers ────────────────────────────────────────────

fn load_blacklist() -> Result<BlacklistConfig, Box<dyn Error + Send + Sync>> {
    if !Path::new(BLACKLIST_FILE).exists() {
        return Ok(BlacklistConfig::default());
    }
    let raw = fs::read_to_string(BLACKLIST_FILE)?;
    Ok(toml::from_str(&raw)?)
}

fn save_blacklist(config: &BlacklistConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::write(BLACKLIST_FILE, toml::to_string(config)?)?;
    Ok(())
}

fn load_keeplist() -> Result<KeepConfig, Box<dyn Error + Send + Sync>> {
    if !Path::new(KEEPLIST_FILE).exists() {
        return Ok(KeepConfig { defaults: HashMap::default(), items: HashMap::default() });
    }
    let raw = fs::read_to_string(KEEPLIST_FILE)?;
    Ok(toml::from_str(&raw)?)
}

fn get_keep_quantity(
    keeplist: &KeepConfig,
    slug: &str,
    rank: Option<u8>,
    category: &str,
) -> u32 {
    if let Some(rules) = keeplist.items.get(slug) {
        if let Some(rank_val) = rank
            && let Some(rule) = rules.iter().find(|r| r.rank == Some(rank_val))
        {
            return rule.keep;
        }
        if let Some(rule) = rules.iter().find(|r| r.rank.is_none()) {
            return rule.keep;
        }
    }
    if let Some(rule) = keeplist.defaults.get(category) {
        return rule.keep;
    }
    0
}

fn add_to_keeplist(
    keeplist: &mut KeepConfig,
    slug: &str,
    rank: Option<u8>,
    qty: u32,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let rules = keeplist.items.entry(slug.to_string()).or_default();
    rules.retain(|r| r.rank != rank);
    rules.push(KeepRule { keep: qty, rank });
    fs::write(KEEPLIST_FILE, toml::to_string(keeplist)?)?;
    Ok(())
}

#[cfg(test)]
mod price_conflict_tests {
    use super::*;
    use crate::wfm_client::Order;

    #[test]
    fn does_not_match_across_different_ranks_at_same_price() {
        let mut map: HashMap<ListingKey, Vec<Order>> = HashMap::new();
        let item_id = "abc".to_string();

        let key0 = ListingKey { item_id: item_id.clone(), rank: Some(0) };
        let order0 = Order { id: "o0".into(), order_type: "sell".into(), platinum: 50, quantity: 1, item_id: item_id.clone(), visible: true, rank: Some(0), subtype: None };
        map.entry(key0).or_default().push(order0);

        let key5 = ListingKey { item_id: item_id.clone(), rank: Some(5) };
        let order5 = Order { id: "o5".into(), order_type: "sell".into(), platinum: 50, quantity: 1, item_id: item_id.clone(), visible: true, rank: Some(5), subtype: None };
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
        assert!(matches!(decide_no_op(1, 1, 100, 100), NoOpDecision::TrueNoOp));
    }

    #[test]
    fn restock_with_stable_price_is_quantity_sync_only() {
        // 105 owned (100 listed + 5 new), price unchanged at 1p.
        assert!(matches!(
            decide_no_op(1, 1, 105, 100),
            NoOpDecision::QuantitySyncOnly { new_quantity: 105, .. }
        ));
    }

    #[test]
    fn real_price_move_needs_review() {
        // existing listed at 40p, market now suggests 55p — well outside 3% tolerance.
        assert!(matches!(decide_no_op(55, 40, 10, 10), NoOpDecision::NeedsReview));
    }

    #[test]
    fn small_drift_within_tolerance_is_still_noop() {
        // 41p existing vs 42p suggested on a price where 3% tolerance is >= 1.
        assert!(matches!(decide_no_op(42, 41, 10, 10), NoOpDecision::TrueNoOp));
    }
}

#[cfg(test)]
mod threshold_calibration_tests {
    use super::*;
    use crate::models::WfmStatsResponse;
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

#[cfg(test)]
mod set_aggregation_tests {
    use super::*;
    use crate::models::{WfmItem, WfmI18n, WfmEn, WfcdItem};
    use std::collections::HashMap;

    fn build_test_wfm_item(slug: &str, name: &str) -> WfmItem {
        WfmItem {
            id: slug.to_string(),
            slug: slug.to_string(),
            game_ref: None,
            tags: vec![],
            max_rank: None,
            i18n: WfmI18n { en: WfmEn { name: name.to_string() } },
            subtypes: vec![],
            set_root: true,
            bulk_tradable: false,
            max_amber_stars: None,
            max_cyan_stars: None,
        }
    }

    fn build_test_candidate(slug: &str, name: &str, game_ref: &str, qty: u32) -> MappedItem {
        MappedItem {
            id: slug.to_string(),
            slug: slug.to_string(),
            name: name.to_string(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: "Common".to_string(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: game_ref.to_string(),
            subtypes: vec![],
            bulk_tradable: false,
        }
    }

    #[test]
    fn exactly_one_set() {
        // Build: Mag Prime requires BP, Chassis, Neuroptics, Systems (1 each)
        let build_name = "Mag Prime";
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();

        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(build_unique.clone(), WfcdItem {
            unique_name: build_unique.clone(),
            name: build_name.to_string(),
            level_stats: None,
            category: None,
            rarity: None,
            fusion_limit: None,
            components: None,
        });

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            ("/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(), 1),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        // WFM by name: set item and component items
        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("mag prime set".to_string(), build_test_wfm_item("mag_prime_set", "Mag Prime Set"));
        wfm_by_name.insert("mag prime blueprint".to_string(), build_test_wfm_item("mag_prime_blueprint", "Mag Prime Blueprint"));
        wfm_by_name.insert("mag prime chassis".to_string(), build_test_wfm_item("mag_prime_chassis", "Mag Prime Chassis"));
        wfm_by_name.insert("mag prime neuroptics".to_string(), build_test_wfm_item("mag_prime_neuroptics", "Mag Prime Neuroptics"));
        wfm_by_name.insert("mag prime systems".to_string(), build_test_wfm_item("mag_prime_systems", "Mag Prime Systems"));

        // Parent map: each component maps to the build
        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        // Candidates: one of each component
        let candidates = vec![
            build_test_candidate("mag_prime_blueprint", "Mag Prime Blueprint", "/Lotus/Types/Recipes/Components/MagPrimeBlueprint", 1),
            build_test_candidate("mag_prime_chassis", "Mag Prime Chassis", "/Lotus/Types/Recipes/Components/MagPrimeChassis", 1),
            build_test_candidate("mag_prime_neuroptics", "Mag Prime Neuroptics", "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics", 1),
            build_test_candidate("mag_prime_systems", "Mag Prime Systems", "/Lotus/Types/Recipes/Components/MagPrimeSystems", 1),
        ];

        // Prices: set price 100, each component 10
        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 10.0);
        prices.insert("mag_prime_systems".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Expect exactly one set item, no leftover components
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].slug, "mag_prime_set");
        assert_eq!(result[0].quantity, 1);
    }

    #[test]
    fn two_sets_with_leftovers() {
        // Same as above but with extra parts
        // Build: Mag Prime (1 each)
        // Candidates: 2 BP, 2 Chassis, 2 Neuroptics, 5 Systems
        // Expect 2 sets, leftover 3 Systems

        let build_name = "Mag Prime";
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();

        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(build_unique.clone(), WfcdItem {
            unique_name: build_unique.clone(),
            name: build_name.to_string(),
            level_stats: None,
            category: None,
            rarity: None,
            fusion_limit: None,
            components: None,
        });

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            ("/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(), 1),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("mag prime set".to_string(), build_test_wfm_item("mag_prime_set", "Mag Prime Set"));
        wfm_by_name.insert("mag prime blueprint".to_string(), build_test_wfm_item("mag_prime_blueprint", "Mag Prime Blueprint"));
        wfm_by_name.insert("mag prime chassis".to_string(), build_test_wfm_item("mag_prime_chassis", "Mag Prime Chassis"));
        wfm_by_name.insert("mag prime neuroptics".to_string(), build_test_wfm_item("mag_prime_neuroptics", "Mag Prime Neuroptics"));
        wfm_by_name.insert("mag prime systems".to_string(), build_test_wfm_item("mag_prime_systems", "Mag Prime Systems"));

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        let candidates = vec![
            build_test_candidate("mag_prime_blueprint", "Mag Prime Blueprint", "/Lotus/Types/Recipes/Components/MagPrimeBlueprint", 2),
            build_test_candidate("mag_prime_chassis", "Mag Prime Chassis", "/Lotus/Types/Recipes/Components/MagPrimeChassis", 2),
            build_test_candidate("mag_prime_neuroptics", "Mag Prime Neuroptics", "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics", 2),
            build_test_candidate("mag_prime_systems", "Mag Prime Systems", "/Lotus/Types/Recipes/Components/MagPrimeSystems", 5),
        ];

        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 10.0);
        prices.insert("mag_prime_systems".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Expect 2 sets + 1 leftover systems (qty 3)
        assert_eq!(result.len(), 2);
        let sets: Vec<_> = result.iter().filter(|i| i.slug == "mag_prime_set").collect();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].quantity, 2);
        let leftovers: Vec<_> = result.iter().filter(|i| i.slug == "mag_prime_systems").collect();
        assert_eq!(leftovers.len(), 1);
        assert_eq!(leftovers[0].quantity, 3);
    }

    #[test]
    fn not_enough_parts_no_set() {
        // Missing one component -> no set
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();
        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(build_unique.clone(), WfcdItem {
            unique_name: build_unique.clone(),
            name: "Mag Prime".to_string(),
            level_stats: None,
            category: None,
            rarity: None,
            fusion_limit: None,
            components: None,
        });

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            ("/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(), 1),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("mag prime set".to_string(), build_test_wfm_item("mag_prime_set", "Mag Prime Set"));

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        let candidates = vec![
            build_test_candidate("mag_prime_blueprint", "Mag Prime Blueprint", "/Lotus/Types/Recipes/Components/MagPrimeBlueprint", 1),
            build_test_candidate("mag_prime_chassis", "Mag Prime Chassis", "/Lotus/Types/Recipes/Components/MagPrimeChassis", 1),
            build_test_candidate("mag_prime_neuroptics", "Mag Prime Neuroptics", "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics", 1),
            // missing systems
        ];

        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // No set, all parts remain as individual candidates
        assert_eq!(result.len(), 3);
        assert!(!result.iter().any(|i| i.slug == "mag_prime_set"));
    }

    #[test]
    fn expensive_component_does_not_prevent_bundling() {
        // Regression test for the Corinth/Akvasto/Phantasma Prime bug: a component priced
        // well above the assembled set's price (e.g. the part sourced from the rarest-tier
        // relic) must NOT block set formation. Warframe's Prime part market routinely prices
        // one component above 50%, even above 100%, of the set's own price — that's normal
        // relic-scarcity pricing, not a signal the bundle is a bad trade. A guard that killed
        // the whole set over this used to silently drop sets (and their components) from the
        // run entirely — see session log: "Set 'Corinth Prime' skipped: component 'Corinth
        // Prime Barrel' priced 45.3p exceeds 50% of set price 88.2p".
        let build_unique = "/Lotus/Types/Recipes/WarframeRecipes/MagPrime".to_string();
        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(build_unique.clone(), WfcdItem {
            unique_name: build_unique.clone(),
            name: "Mag Prime".to_string(),
            level_stats: None,
            category: None,
            rarity: None,
            fusion_limit: None,
            components: None,
        });

        let mut requirements = BuildRequirements::new();
        let recipe = vec![
            ("/Lotus/Types/Recipes/Components/MagPrimeBlueprint".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeChassis".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeNeuroptics".to_string(), 1),
            ("/Lotus/Types/Recipes/Components/MagPrimeSystems".to_string(), 1),
        ];
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("mag prime set".to_string(), build_test_wfm_item("mag_prime_set", "Mag Prime Set"));

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        let candidates = vec![
            build_test_candidate("mag_prime_blueprint", "Mag Prime Blueprint", "/Lotus/Types/Recipes/Components/MagPrimeBlueprint", 1),
            build_test_candidate("mag_prime_chassis", "Mag Prime Chassis", "/Lotus/Types/Recipes/Components/MagPrimeChassis", 1),
            build_test_candidate("mag_prime_neuroptics", "Mag Prime Neuroptics", "/Lotus/Types/Recipes/Components/MagPrimeNeuroptics", 1),
            build_test_candidate("mag_prime_systems", "Mag Prime Systems", "/Lotus/Types/Recipes/Components/MagPrimeSystems", 1),
        ];

        let mut prices = HashMap::new();
        prices.insert("mag_prime_set".to_string(), 100.0);
        prices.insert("mag_prime_blueprint".to_string(), 10.0);
        prices.insert("mag_prime_chassis".to_string(), 10.0);
        prices.insert("mag_prime_neuroptics".to_string(), 60.0); // 60% of the 100p set price
        prices.insert("mag_prime_systems".to_string(), 10.0);

        let result = aggregate_sets_with_prices(
            candidates,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Set forms despite one component being priced above 50% of the set's price.
        let sets: Vec<_> = result.iter().filter(|i| i.slug == "mag_prime_set").collect();
        assert_eq!(sets.len(), 1, "a complete set must form even with a disproportionately priced component");
        assert_eq!(sets[0].quantity, 1);
    }

    #[test]
    fn filter_candidates_does_not_starve_aggregation_of_plainly_named_parts() {
        // Regression test for the Lato Vandal bug: Barrel/Receiver components whose display
        // names contain none of "prime"/"set"/"blueprint" (true of most non-Prime weapon
        // parts) used to get dropped by filter_candidates() before aggregate_sets_with_prices
        // ever ran, so a genuinely complete set could never be detected — its Blueprint (the
        // one component whose name happens to say "blueprint") would sit as a 100%
        // "unconsumed" leftover forever, and the Barrel/Receiver vanished entirely, never
        // shown at all. This exercises the real pipeline: filter_candidates() -> the aggregator.
        let build_name = "Lato Vandal";
        let build_unique = "/Lotus/Weapons/Tenno/Pistol/LatoVandal".to_string();

        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert(build_unique.clone(), WfcdItem {
            unique_name: build_unique.clone(),
            name: build_name.to_string(),
            level_stats: None,
            category: None,
            rarity: None,
            fusion_limit: None,
            components: None,
        });

        let recipe = vec![
            ("/Lotus/Types/Recipes/Weapons/LatoVandalBlueprint".to_string(), 1),
            ("/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalBarrel".to_string(), 1),
            ("/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalReceiver".to_string(), 1),
        ];
        let mut requirements = BuildRequirements::new();
        requirements.insert(build_unique.clone(), recipe.clone());

        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("lato vandal set".to_string(), build_test_wfm_item("lato_vandal_set", "Lato Vandal Set"));

        let mut parent_map = BuildParentMap::new();
        for (comp, _) in &recipe {
            parent_map.insert(comp.clone(), build_unique.clone());
        }

        // Matches the real report exactly: Barrel x1, Receiver x3, Blueprint x4 — none of
        // "Barrel"/"Receiver" contain prime/set/blueprint in their names.
        let owned = vec![
            build_test_candidate("lato_vandal_blueprint", "Lato Vandal Blueprint", "/Lotus/Types/Recipes/Weapons/LatoVandalBlueprint", 4),
            build_test_candidate("lato_vandal_barrel", "Lato Vandal Barrel", "/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalBarrel", 1),
            build_test_candidate("lato_vandal_receiver", "Lato Vandal Receiver", "/Lotus/Types/Recipes/Weapons/WeaponParts/LatoVandalReceiver", 3),
        ];

        // Run through filter_candidates() first, exactly like the real pipeline does.
        let filtered = filter_candidates(owned, &parent_map);

        let mut prices = HashMap::new();
        prices.insert("lato_vandal_set".to_string(), 30.0);
        prices.insert("lato_vandal_blueprint".to_string(), 9.0);
        prices.insert("lato_vandal_barrel".to_string(), 9.0);
        prices.insert("lato_vandal_receiver".to_string(), 9.0);

        let result = aggregate_sets_with_prices(
            filtered,
            &parent_map,
            &requirements,
            &wfcd_by_ref,
            &wfm_by_name,
            &prices,
        );

        // Exactly 1 complete set formed (Barrel is the bottleneck at qty 1), plus 3 leftover
        // Blueprints (4 owned - 1 consumed). Leftover Receivers (2 spare) are pruned by the
        // post-aggregation "worth reviewing" heuristic, same as before this fix — the point of
        // this test is that the Set itself now forms and the Blueprint leftover is correctly
        // reduced, not left at the full unconsumed count of 4.
        let sets: Vec<_> = result.iter().filter(|i| i.slug == "lato_vandal_set").collect();
        assert_eq!(sets.len(), 1, "a complete Lato Vandal Set must be detected");
        assert_eq!(sets[0].quantity, 1);

        let blueprint_leftover: Vec<_> = result.iter().filter(|i| i.slug == "lato_vandal_blueprint").collect();
        assert_eq!(blueprint_leftover.len(), 1);
        assert_eq!(blueprint_leftover[0].quantity, 3, "3 spare blueprints should remain after 1 is consumed into the set");
    }
}
