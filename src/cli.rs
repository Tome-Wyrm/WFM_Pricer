use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tokio::time::{sleep, Duration};

use crate::wfm_client::{WfmClient, Credentials, CreateOrder, UpdateOrder, Order as OwnedOrder};
use crate::config::{KEEPLIST_FILE, BLACKLIST_FILE};
use crate::models::{MappedItem, KeepConfig, KeepRule, BlacklistConfig};
use crate::pricing::{
    calculate_saturation_ratio, calculate_weighted_average, derive_endo_to_plat_from_mods,
    fetch_statistics, get_ayatan_endo_yield, is_antique, get_fusion_cost_from_zero,
};

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
}

fn arcane_rank_cost(rank: u8) -> u32 {
    match rank {
        1 => 3,
        2 => 6,
        3 => 10,
        4 => 15,
        5 => 21,
        _ => 1,
    }
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

fn print_header(title: &str) {
    println!("\x1B[1;36m================================================================================\x1B[0m");
    println!("\x1B[1;35m   {}   \x1B[0m", title.to_uppercase());
    println!("\x1B[1;36m================================================================================\x1B[0m");
}

fn print_info(label: &str, value: &str) {
    println!("\x1B[1;34m  {label:<25}\x1B[0m : \x1B[32m{value}\x1B[0m");
}

fn print_warning(msg: &str) {
    println!("\x1B[1;33m  [WARNING] {msg}\x1B[0m");
}

#[allow(dead_code)]
fn print_error_ui(msg: &str) {
    println!("\x1B[1;31m  [ERROR] {msg}\x1B[0m");
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
    println!("Fetching your active listings from Warframe.Market...");
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

fn filter_candidates(mapped_items: Vec<MappedItem>) -> Vec<MappedItem> {
    println!("Filtering high-value candidates for trade review...");
    mapped_items
        .into_iter()
        .filter(|item| {
            if item.is_arcane || item.is_ayatan {
                return true;
            }
            if item.is_mod {
                if let Some(mr) = item.max_rank && mr >= 5 {
                    return true;
                }
                return false;
            }
            let name_lower = item.name.to_lowercase();
            name_lower.contains("prime") || name_lower.contains("set") || name_lower.contains("blueprint")
        })
        .collect()
}

async fn build_priced_candidates(
    candidates: Vec<MappedItem>,
    endo_rate: f64,
) -> (Vec<(MappedItem, f64, f64, u32, f64)>, Vec<(String, f64, u32, u32, f64)>) {
    let mut priced = Vec::new();
    let mut upgrades = Vec::new();

    for c in candidates {
        if let Ok(stats) = fetch_statistics(&c.slug).await {
            let target_rank = if c.is_mod || c.is_arcane { c.rank } else { None };
            let (wa_price, _) = calculate_weighted_average(&stats, target_rank);
            let (r0_price, _) = calculate_weighted_average(&stats, Some(0));
            let is_antique = is_antique(&c.slug, &c.game_ref);
            let current_rank_u32 = u32::from(c.rank.unwrap_or(0));
            let endo_cost = get_fusion_cost_from_zero(&c.rarity, current_rank_u32, is_antique);
            let ninety_days_closed = &stats.payload.statistics_closed.ninety_days;
            let (vol_30d, _trading_days_30d) = recent_volume(&stats, target_rank, 30);
            let score = wa_price * (1.0 + f64::from(vol_30d)).ln();
            let profit = wa_price - r0_price;
            let ppe = if endo_cost > 0 { (profit / f64::from(endo_cost)) * 1000.0 } else { 0.0 };
            priced.push((c.clone(), wa_price, calculate_saturation_ratio(&stats, target_rank), vol_30d, score));

            let is_maxed = c.rank.zip(c.max_rank).is_some_and(|(r, mr)| r >= mr);
            if endo_cost > 0 && ppe > endo_rate * 1000.0 && c.quantity > 0 && !is_maxed {
                println!("\x1B[1;32m[!] PROFITABLE UPGRADE\x1B[0m: {} (PPE: {:.2})", c.name, ppe);
            }
            if c.is_mod && !is_maxed && let Some(mr) = c.max_rank {
                let (max_rank_price, _) = calculate_weighted_average(&stats, Some(mr));
                let delta = max_rank_price - wa_price;
                let endo_to_max = get_fusion_cost_from_zero(&c.rarity, u32::from(mr), is_antique)
                    .saturating_sub(endo_cost);
                if delta > 0.0 && endo_to_max > 0 {
                    let upgrade_score = (delta / f64::from(endo_to_max)) * (1.0 + f64::from(vol_30d)).ln();
                    upgrades.push((c.name.clone(), delta, endo_to_max, vol_30d, upgrade_score));
                }
            }
        }
    }
    (priced, upgrades)
}

fn print_upgrade_suggestions(suggestions: &[(String, f64, u32, u32, f64)]) {
    let mut sorted = suggestions.to_vec();
    sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(15);

    print_header("Mod Upgrade Suggestions (Best Endo Value × Volume)");
    println!("\x1B[1m  {:<35} | {:<14} | {:<12} | {:<10} | Score\x1B[0m",
        "Mod", "Δ Plat (→max)", "Endo Cost", "30d Vol");
    println!("  {}", "-".repeat(82));
    for (name, delta, endo, vol, score) in &sorted {
        println!("  {name:<35} | {delta:<14.1} | {endo:<12} | {vol:<10} | {score:.4}");
    }
    println!();
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

async fn handle_single_candidate(
    mut item: MappedItem,
    wa_price: f64,
    saturation: f64,
    vol_30d: u32,
    ctx: &mut CandidateContext<'_>,
) -> Result<Option<SessionReportItem>, Box<dyn Error + Send + Sync>> {
    if ctx.blacklist_set.slugs.contains(&item.slug) {
        return Ok(None);
    }

    let keep_copies = get_keep_quantity(ctx.keeplist, &item.slug, item.rank, item.category());
    if keep_copies > 0 {
        if item.quantity <= keep_copies { return Ok(None); }
        item.quantity -= keep_copies;
    }

    if item.is_ayatan && let Some(endo_yield) = get_ayatan_endo_yield(&item.slug) {
        let endo_value = f64::from(endo_yield) * ctx.endo_rate;
        if wa_price < endo_value * 1.15 {
            println!("[SKIP] {} worth {:.1}p as Endo (vs {:.1}p market)", item.name, endo_value, wa_price);
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
            .map(|l| if item.is_arcane {
                arcane_rank_cost(l.rank.unwrap_or(0)) * l.quantity()
            } else {
                l.quantity()
            })
            .sum()
    });

    let available_qty = item.quantity.saturating_sub(listed_qty);
    let is_already_listed = matching_listings.is_some();

    if available_qty == 0 { return Ok(None); }

    if *ctx.active_slots_count >= 100 && !is_already_listed {
        print_warning(&format!("Budget limit reached (100/100 slots). Skipping listing creation candidate: {}", item.name));
        return Ok(None);
    }

    println!("\x1B[1;36m--------------------------------------------------------------------------------\x1B[0m");
    println!("\x1B[1mCANDIDATE\x1B[0m: \x1B[1;32m{}\x1B[0m | Slug: {} | Qty Available: {}", item.name, item.slug, available_qty);
    println!("  Rank: {:<5} | 30d Vol: {:<6} | Est Price (WA): \x1B[1;33m{:.1} plat\x1B[0m", item.rank.unwrap_or(0), vol_30d, wa_price);
    println!("  Saturation Ratio: {saturation:.3} (sell volume vs closed volume)");
    println!("  Already Listed on WFM: {}", if is_already_listed { "\x1B[1;32mYES\x1B[0m" } else { "\x1B[31mNO\x1B[0m" });

    if is_already_listed && let Some(listings) = ctx.existing_listings_map.get(&listing_key) {
        for (idx, listing) in listings.iter().enumerate() {
            println!("    [{}] Listed price: {} plat | Qty listed: {} | Visible: {}", idx + 1, listing.platinum(), listing.quantity(), listing.is_visible());
        }
    }

    print!("\x1B[1;35m  Action? [Y] List/Update | [N] Skip | [K] Add to Keep List | [B] Blacklist | [X] Save & Exit: \x1B[0m");
    let _ = ctx.stdout.flush();
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let choice = choice.trim().to_uppercase();

    if choice == "X" {
        return Err("EXIT_REQUESTED".into());
    }

    if choice == "B" {
        println!("Blacklisting {} permanently...", item.name);
        ctx.blacklist_set.slugs.insert(item.slug.clone());
        save_blacklist(ctx.blacklist_set)?;
        return Ok(None);
    }

    if choice == "K" {
        print!("\x1B[1;34m  How many copies of {} (rank {}) do you want to keep? \x1B[0m", item.name, item.rank.unwrap_or(0));
        let _ = ctx.stdout.flush();
        let mut keep_str = String::new();
        io::stdin().read_line(&mut keep_str)?;
        if let Ok(keep_qty) = keep_str.trim().parse::<u32>() {
            add_to_keeplist(ctx.keeplist, &item.slug, item.rank, keep_qty)?;
            println!("Saved to keeplist.json!");
        }
        return Ok(None);
    }

    if choice == "Y" {
        return handle_list_or_update(
            &item,
            wa_price,
            available_qty,
            is_already_listed,
            &listing_key,
            ctx,
        ).await;
    }

    Ok(None)
}

async fn handle_list_or_update(
    item: &MappedItem,
    wa_price: f64,
    available_qty: u32,
    is_already_listed: bool,
    listing_key: &ListingKey,
    ctx: &mut CandidateContext<'_>,
) -> Result<Option<SessionReportItem>, Box<dyn Error + Send + Sync>> {
    // Price prompt
    print!("  Price to list (default {wa_price:.1}): ");
    let _ = ctx.stdout.flush();
    let mut price_str = String::new();
    io::stdin().read_line(&mut price_str)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let default_price = wa_price.round() as u32;
    let price: u32 = price_str.trim().parse::<u32>().unwrap_or(default_price);

    // Quantity prompt
    print!("  Quantity to list (default {available_qty}): ");
    let _ = ctx.stdout.flush();
    let mut qty_str = String::new();
    io::stdin().read_line(&mut qty_str)?;
    let quantity: u32 = qty_str.trim().parse::<u32>().unwrap_or(available_qty);

    let mut cyan_stars: Option<u8> = None;
    let mut amber_stars: Option<u8> = None;
    let mut per_trade: Option<u32> = None;

    // ── Price‑conflict detection ──────────────────────────────────────────────
    let mut existing_same_price_order: Option<&OwnedOrder> = None;
    for (key, orders) in ctx.existing_listings_map.iter() {
        if key.item_id == item.id {
            if let Some(order) = orders.iter().find(|o| o.platinum() == price) {
                existing_same_price_order = Some(order);
                break;
            }
        }
    }

    if let Some(order) = existing_same_price_order {
        println!("\x1B[33m[SYNC] Found an existing order for {} at the same price ({} plat). Updating its quantity to {}...\x1B[0m",
            item.name, price, quantity);
        let update = UpdateOrder::new().platinum(price).quantity(quantity);
        match ctx.wfm_client.update_order(order.id(), update).await {
            Ok(()) => {
                println!("\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m", item.name);
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
                eprintln!("\x1B[31m[SYNC_ERROR] Failed to update existing order: {}\x1B[0m", e);
                return Ok(None);
            }
        }
    }

    // ── Ayatan star prompts ───────────────────────────────────────────────────
    if item.is_ayatan {
        per_trade = Some(1);
        if item.slug.ends_with("_sculpture") {
            let (max_cyan, max_amber) = ayatan_max_stars(&item.slug);
            print!("  Cyan Stars installed (default {max_cyan}): ");
            let _ = ctx.stdout.flush();
            let mut c_str = String::new();
            io::stdin().read_line(&mut c_str)?;
            cyan_stars = Some(c_str.trim().parse::<u8>().unwrap_or(max_cyan));

            print!("  Amber Stars installed (default {max_amber}): ");
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
            println!("\x1B[33m[SYNC] Updating listing: {} to {} plat...\x1B[0m", item.name, price);
            sleep(Duration::from_millis(400)).await;
            let update = UpdateOrder::new().platinum(price).quantity(quantity);
            match ctx.wfm_client.update_order(first_listing.id(), update).await {
                Ok(()) => {
                    println!("\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m", item.name);
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
                    eprintln!("\x1B[31m[SYNC_ERROR] Failed to update listing {}: {}\x1B[0m", first_listing.id(), e);
                    return Ok(None);
                }
            }
        }
    } else {
        let rank_opt = if item.is_mod || item.is_arcane { item.rank } else { None };
        println!("\x1B[33m[SYNC] Posting listing: {} (rank: {:?}) for {} plat...\x1B[0m", item.name, rank_opt, price);
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
            println!("This item supports subtypes: {:?}", item.subtypes);
            print!("Choose subtype (default {}): ", item.subtypes[0]);
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
            if let Some(pt) = per_trade {
                order = order.with_per_trade(pt);
            }
        }

        match ctx.wfm_client.create_order(order).await {
            Ok(()) => {
                println!("\x1B[32m[SYNC] Successfully listed {} x{}!\x1B[0m", item.name, quantity);
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
                eprintln!("\x1B[31m[SYNC_ERROR] Failed to list {}: {}\x1B[0m", item.name, e);
                return Ok(None);
            }
        }
    }

    Ok(None)
}

async fn process_candidates(
    priced_candidates: Vec<(MappedItem, f64, f64, u32, f64)>,
    existing_listings_map: &HashMap<ListingKey, Vec<OwnedOrder>>,
    wfm_client: &WfmClient,
    endo_rate: f64,
    blacklist_set: &mut BlacklistConfig,
    keeplist: &mut KeepConfig,
    active_slots_count: &mut usize,
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
pub async fn run_cli(mapped_items: Vec<MappedItem>) -> Result<(), Box<dyn Error + Send + Sync>> {
    print_header("Warframe.Market Advisor Session Init");

    let (email, password) = load_credentials()?;
    let creds = Credentials::new(&email, &password, Credentials::generate_device_id());
    let wfm_client = WfmClient::from_credentials(creds).await?;
    let username = wfm_client.get_username().await?;

    let (user_listings, existing_listings_map) = fetch_user_listings(&wfm_client).await?;
    let candidates = filter_candidates(mapped_items);
    println!("Identified {} tradeable high-value candidates.", candidates.len());

    println!("Deriving dynamic Endo exchange rate from Ayatan prices...");
    let endo_rate = derive_endo_to_plat_from_mods().await;
    print_info("Derived Endo Rate", &format!("{:.5} plat/endo (or {:.1} plat per 1000 endo)", endo_rate, endo_rate * 1000.0));

    print_header("Trade Candidate Evaluation");
    println!("Fetching WFM pricing and volume stats dynamically for candidates...");

    let (priced_candidates, upgrade_suggestions) = build_priced_candidates(candidates, endo_rate).await;
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
    ).await?;

    let report = SessionReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        username,
        items_processed: session_items,
    };
    write_session_report(&report)?;

    print_header("WFM Pricer Session Completed Successfully");
    println!("Session report written to: 'session_report.json'");

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
