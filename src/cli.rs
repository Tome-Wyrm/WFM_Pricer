// use tokio::sync::mpsc;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tokio::time::{sleep, Duration};

use wf_market::{Client, Credentials, CreateOrder, UpdateOrder};
use wf_market::models::OwnedOrder;
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
    pub action: String, // "Created" or "Updated"
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
/// Returns the (max_cyan, max_amber) star capacity for a fully-filled Ayatan sculpture.
fn ayatan_max_stars(slug: &str) -> (u8, u8) {
    match slug {
        "ayatan_anasa_sculpture"    => (2, 2),
        "ayatan_ayr_sculpture"      => (3, 0),
        "ayatan_chattraka_sculpture"=> (2, 1),
        "ayatan_hemakara_sculpture" => (2, 1),
        "ayatan_kitha_sculpture"    => (4, 1),
        "ayatan_orta_sculpture"     => (3, 1),
        "ayatan_piv_sculpture"      => (2, 1),
        "ayatan_sah_sculpture"      => (2, 1),
        "ayatan_valana_sculpture"   => (2, 1),
        "ayatan_vaya_sculpture"     => (2, 1),
        "ayatan_zambuka_sculpture"  => (2, 1),
        _                           => (0, 0),
    }
}


/// The CLI UI dashboard printer. Uses ANSI colors for high visual premium appeal.
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

/// Runs the interactive CLI loop.
#[allow(clippy::too_many_lines)] // This function needs a significant refactor to reduce line count.
/// # Errors
/// Returns an error if CLI input/output operations fail, WFM client operations fail,
/// or file operations for blacklist/keeplist fail.
///
/// # Panics
/// Panics if `serde_json::Value` is not an object when expected (e.g., `unwrap()`).
pub async fn run_cli(mapped_items: Vec<MappedItem>) -> Result<(), Box<dyn Error + Send + Sync>> {
    print_header("Warframe.Market Advisor Session Init");

    // 1. Load credentials from environment
    let email = std::env::var("WFM_EMAIL").unwrap_or_default();
    let password = std::env::var("WFM_PASSWORD").unwrap_or_default();

    if email.is_empty() || password.is_empty() {
        print_warning("WFM_EMAIL or WFM_PASSWORD not found in environment.");
        print_info("Please supply them", "e.g., set WFM_EMAIL=email in environment or .env file.");
        return Err("Missing credentials".into());
    }

    // 2. Initialize WFM Client and sign in
    let creds = Credentials::new(&email, &password, Credentials::generate_device_id());
    let wfm_client = Client::from_credentials(creds).await?;
    // Fetch username via raw JSON to work around a crate deserialization bug:
    // the API may return `"theme": ""` which doesn't match the crate's Theme enum
    // variants ("light" | "dark" | "system"), causing a hard parse error.
    let username = {
        let resp = reqwest::Client::new()
            .get("https://api.warframe.market/v2/me")
            .header("Authorization", format!("Bearer {}", wfm_client.token()))
            .header("Platform",   "pc")
            .header("Language",   "en")
            .header("Crossplay",  "true")
            .header("User-Agent", "wfm-pricer-cli")
            .send()
            .await
            .map_err(|e| format!("Failed to fetch user profile: {e}"))?;
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse user profile: {e}"))?;
        val["data"]["ingameName"]
            .as_str()
            .unwrap_or("unknown")
            .to_string()
    };

    // 3. Retrieve user listings and track budget slot limit (100)
    println!("Fetching your active listings from Warframe.Market...");
    let all_orders = wfm_client.my_orders().await?;
    let user_listings: Vec<OwnedOrder> = all_orders.into_iter().filter(|o| o.is_sell()).collect();
    let current_listing_count = user_listings.len();
    print_info("Active Listings on WFM", &format!("{current_listing_count}/100 slots used"));

    // Map existing listings by item_id+rank for quick lookup/budget calculation
    let mut existing_listings_map: HashMap<ListingKey, Vec<OwnedOrder>> = HashMap::new();

    for listing in user_listings {
        existing_listings_map
            .entry(ListingKey {
                item_id: listing.item_id().to_string(),
                rank: listing.order.rank,
            })
            .or_default()
            .push(listing);
    }

    // 4. Filtering high-value candidates to keep startup and API usage low and focused.
    println!("Filtering high-value candidates for trade review...");
    let candidates: Vec<MappedItem> = mapped_items
        .into_iter()
        .filter(|item| {
            if item.is_arcane || item.is_ayatan {
                return true;
            }
            if item.is_mod {
                if let Some(mr) = item.max_rank
                    && mr >= 5
                {
                    return true;
                }
                return false;
            }
            // Prime parts & Sets
            let name_lower = item.name.to_lowercase();
            name_lower.contains("prime") || name_lower.contains("set") || name_lower.contains("blueprint")
        })
        .collect();

    println!("Identified {} tradeable high-value candidates.", candidates.len());

    // 5. Pre-fetch JIT statistics for Ayatan arbitrage report (First 15 Ayatans/Stars or general candidates)
    println!("Deriving dynamic Endo exchange rate from Ayatan prices...");
    let endo_rate = derive_endo_to_plat_from_mods().await;
    print_info("Derived Endo Rate", &format!("{:.5} plat/endo (or {:.1} plat per 1000 endo)", endo_rate, endo_rate * 1000.0));

    // Build the Ayatan arbitrage report
    print_header("Ayatan Arbitrage Report");
    let all_ayatans = vec![
        "ayatan_cyan_star", "ayatan_amber_star", "ayatan_anasa_sculpture",
        "ayatan_ayr_sculpture", "ayatan_orta_sculpture", "ayatan_sah_sculpture",
        "ayatan_valana_sculpture", "ayatan_vaya_sculpture", "ayatan_piv_sculpture",
        "ayatan_hemakara_sculpture"
    ];

    println!("\x1B[1m  {:<30} | {:<12} | {:<10} | {:<12} | {:<10}\x1B[0m", "Ayatan / Star Slug", "Endo Yield", "Est Plat", "Avg Buy Ord", "Arbitrage?");
    println!("  --------------------------------------------------------------------------------");
    for slug in all_ayatans {
        if let Some(yield_endo) = get_ayatan_endo_yield(slug) {
            let est_plat = f64::from(yield_endo) * endo_rate;
            if let Ok(stats) = fetch_statistics(slug).await {
                // Find latest buy orders from statistics_live
                let avg_buy = stats
                    .payload
                    .statistics_live
                    .ninety_days
                    .iter()
                    .rfind(|d| d.order_type.as_deref() == Some("buy"))
                    .map_or(0.0, |d| d.wa_price);

                let arbitrage = if avg_buy > 0.0 && avg_buy < est_plat - 1.0 {
                    "\x1B[1;32mYES\x1B[0m"
                } else {
                    "\x1B[31mNO\x1B[0m"
                };

                println!("  {slug:<30} | {yield_endo:<12} | {est_plat:<10.2} | {avg_buy:<12.2} | {arbitrage:<10}");
            }
        }
    }
    println!();

    // 6. Setup Session Report Tracking
    let mut session_report_items = Vec::new();

    // 7. Interactive loop and candidate evaluation
    print_header("Trade Candidate Evaluation");
    println!("Fetching WFM pricing and volume stats dynamically for candidates...");

    let mut priced_candidates = Vec::new();
    // Fetch benchmark once outside the loop to save time
    let endo_rate = derive_endo_to_plat_from_mods().await;
    let mut blacklist_set = load_blacklist()?;
    let mut keeplist = load_keeplist()?;
    let mut stdout = io::stdout();

    for c in candidates {
        if let Ok(stats) = fetch_statistics(&c.slug).await {
            // 1. Get Market Prices
            // Consolidate calls: use the target rank logic to get prices
            let target_rank = if c.is_mod || c.is_arcane { c.rank } else { None };

            let (wa_price, _) = calculate_weighted_average(&stats, target_rank);
            let (r0_price, _) = calculate_weighted_average(&stats, Some(0));

            // 2. Calculate Costs
            let is_antique = is_antique(&c.slug, &c.game_ref);
            let target_rank_u32 = u32::from(c.max_rank.unwrap_or(0));
            let endo_cost = get_fusion_cost_from_zero(
                &c.rarity,
                target_rank_u32,
                is_antique
            );

            // 3. Calculate metrics
            let ninety_days_closed = &stats.payload.statistics_closed.ninety_days;
            let vol_30d: u32 = ninety_days_closed.iter()
                .filter(|d| d.mod_rank == target_rank.map(u32::from))
                .take(30)
                .map(|d| d.volume)
                .sum();

            let score = wa_price * (1.0 + f64::from(vol_30d)).ln();

            // Calculate PPE (Profit Per Endo)
            let profit = wa_price - r0_price;
            let ppe = if endo_cost > 0 { (profit / f64::from(endo_cost)) * 1000.0 } else { 0.0 };

            priced_candidates.push((c.clone(), wa_price, calculate_saturation_ratio(&stats, target_rank), vol_30d, score));

            // 4. Print recommendations
            let in_keeplist = get_keep_quantity(&keeplist, &c.slug, c.rank, c.category()) > 0;
            let is_maxed = c.rank.zip(c.max_rank).is_some_and(|(r, mr)| r >= mr);
            if endo_cost > 0 && ppe > endo_rate * 1000.0 && in_keeplist && !is_maxed {
                println!("\x1B[1;32m[!] PROFITABLE UPGRADE\x1B[0m: {} (PPE: {:.2})", c.name, ppe);
            }
        }
    }

    // Sort descending by score (Items already listed are prioritized first)
    priced_candidates.sort_by(|a, b| {
        let a_key = ListingKey { item_id: a.0.id.clone(), rank: if a.0.is_mod || a.0.is_arcane { a.0.rank } else { None } };
        let b_key = ListingKey { item_id: b.0.id.clone(), rank: if b.0.is_mod || b.0.is_arcane { b.0.rank } else { None } };

        let a_listed = existing_listings_map.contains_key(&a_key);
        let b_listed = existing_listings_map.contains_key(&b_key);

        if a_listed && !b_listed { std::cmp::Ordering::Less }
        else if !a_listed && b_listed { std::cmp::Ordering::Greater }
        else { b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal) }
    });

    let mut active_slots_count = current_listing_count;

    for (mut item, wa_price, saturation, vol_30d, _score) in priced_candidates {
        if blacklist_set.slugs.contains(&item.slug) { continue; }

        let keep_copies = get_keep_quantity(&keeplist, &item.slug, item.rank, item.category());
        if keep_copies > 0 {
            if item.quantity <= keep_copies { continue; }
            item.quantity -= keep_copies;
        }

        if item.is_ayatan
            && let Some(endo_yield) = get_ayatan_endo_yield(&item.slug) {
                let endo_value = f64::from(endo_yield) * endo_rate;
                if wa_price < endo_value * 1.15 {
                    println!("[SKIP] {} worth {:.1}p as Endo (vs {:.1}p market)", item.name, endo_value, wa_price);
                    continue;
                }
            }

        let listing_key = ListingKey {
            item_id: item.id.clone(),
            rank: if item.is_mod || item.is_arcane { item.rank } else { None },
        };

        let matching_listings = existing_listings_map.get(&listing_key);
        let listed_qty: u32 = matching_listings.map_or(0, |listings: &Vec<OwnedOrder>| {
            listings.iter().map(|l| {
                if item.is_arcane { arcane_rank_cost(l.order.rank.unwrap_or(0)) * l.quantity() }
                else { l.quantity() }
            }).sum()
        });

        let available_qty = item.quantity.saturating_sub(listed_qty);
        let is_already_listed = matching_listings.is_some();

        if available_qty == 0 { continue; }

        if active_slots_count >= 100 && !is_already_listed {
            print_warning(&format!("Budget limit reached (100/100 slots). Skipping listing creation candidate: {}", item.name));
            continue;
        }

        println!("\x1B[1;36m--------------------------------------------------------------------------------\x1B[0m");
        println!("\x1B[1mCANDIDATE\x1B[0m: \x1B[1;32m{}\x1B[0m | Slug: {} | Qty Available: {}", item.name, item.slug, available_qty);
        println!("  Rank: {:<5} | 30d Vol: {:<6} | Est Price (WA): \x1B[1;33m{:.1} plat\x1B[0m", item.rank.unwrap_or(0), vol_30d, wa_price);
        println!("  Saturation Ratio: {saturation:.3} (sell volume vs closed volume)");
        println!("  Already Listed on WFM: {}", if is_already_listed { "\x1B[1;32mYES\x1B[0m" } else { "\x1B[31mNO\x1B[0m" });

        if is_already_listed && let Some(listings) = existing_listings_map.get(&listing_key) {
            for (idx, listing) in listings.iter().enumerate() {
                println!("    [{}] Listed price: {} plat | Qty listed: {} | Visible: {}", idx + 1, listing.platinum(), listing.quantity(), listing.is_visible());
            }
        }

        print!("\x1B[1;35m  Action? [Y] List/Update | [N] Skip | [K] Add to Keep List | [B] Blacklist | [X] Save & Exit: \x1B[0m");
        let _ = stdout.flush();
        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;
        let choice = choice.trim().to_uppercase();

        if choice == "X" {
            println!("Exiting interactive session...");
            break;
        } else if choice == "B" {
            println!("Blacklisting {} permanently...", item.name);
            blacklist_set.slugs.insert(item.slug.clone());
            save_blacklist(&blacklist_set)?;
        } else if choice == "K" {
            print!("\x1B[1;34m  How many copies of {} (rank {}) do you want to keep? \x1B[0m", item.name, item.rank.unwrap_or(0));
            let _ = stdout.flush();
            let mut keep_str = String::new();
            io::stdin().read_line(&mut keep_str)?;
            if let Ok(keep_qty) = keep_str.trim().parse::<u32>() {
                add_to_keeplist(&mut keeplist, &item.slug, item.rank, keep_qty)?;
                println!("Saved to keeplist.json!");
            }
        } else if choice == "Y" {
            // Prompt price
            print!("  Price to list (default {wa_price:.1}): ");
            let _ = stdout.flush();
            let mut price_str = String::new();
            io::stdin().read_line(&mut price_str)?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let default_price = wa_price.round() as u32;
            let price: u32 = price_str.trim().parse::<u32>().unwrap_or(default_price);

            // Prompt quantity
            print!("  Quantity to list (default {available_qty}): ");
            let _ = stdout.flush();
            let mut qty_str = String::new();
            io::stdin().read_line(&mut qty_str)?;
            let quantity: u32 = qty_str.trim().parse::<u32>().unwrap_or(available_qty);

            let mut cyan_stars: Option<u8> = None;
            let mut amber_stars: Option<u8> = None;
            let mut per_trade: Option<u32> = None;

            // Prompt for missing requirements on Ayatans
            if item.is_ayatan {
                per_trade = Some(1); // Usually required for stackable/Ayatan items
                if item.slug.ends_with("_sculpture") {
                    let (max_cyan, max_amber) = ayatan_max_stars(&item.slug);
                    print!("  Cyan Stars installed (default {max_cyan}): ");
                    let _ = stdout.flush();
                    let mut c_str = String::new();
                    io::stdin().read_line(&mut c_str)?;
                    cyan_stars = Some(c_str.trim().parse::<u8>().unwrap_or(max_cyan));

                    print!("  Amber Stars installed (default {max_amber}): ");
                    let _ = stdout.flush();
                    let mut a_str = String::new();
                    io::stdin().read_line(&mut a_str)?;
                    amber_stars = Some(a_str.trim().parse::<u8>().unwrap_or(max_amber));
                }
            }

            if is_already_listed {
                if let Some(listings) = existing_listings_map.get(&listing_key)
                    && let Some(first_listing) = listings.first() {
                        println!("\x1B[33m[SYNC] Updating listing: {} to {} plat...\x1B[0m", item.name, price);
                        sleep(Duration::from_millis(400)).await; // Respect rate limit
                        let update = UpdateOrder::new().platinum(price).quantity(quantity);
                        match wfm_client.update_order(first_listing.id(), update).await {
                            Ok(_) => {
                                println!("\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m", item.name);
                                session_report_items.push(SessionReportItem {
                                    name: item.name.clone(),
                                    slug: item.slug.clone(),
                                    price,
                                    quantity,
                                    rank: None,
                                    action: "Updated".to_string(),
                                });
                            }
                            Err(e) => {
                                eprintln!("\x1B[31m[SYNC_ERROR] Failed to update listing {}: {}\x1B[0m", first_listing.id(), e);
                            }
                        }
                    }
            } else {
                let rank_opt = if item.is_mod || item.is_arcane { item.rank } else { None };
                println!("\x1B[33m[SYNC] Posting listing: {} (rank: {:?}) for {} plat...\x1B[0m", item.name, rank_opt, price);
                sleep(Duration::from_millis(400)).await; // Respect rate limit

                // Build the order using the CreateOrder builder
                let mut order = CreateOrder::sell(&item.id, price, quantity);
                if let Some(r) = rank_opt {
                    order = order.with_mod_rank(r);
                }
                // Clamp Ayatan star counts to legal maxes
                let (max_cyan, max_amber) = ayatan_max_stars(&item.slug);

                cyan_stars = Some(cyan_stars.unwrap_or(max_cyan).min(max_cyan));
                amber_stars = Some(amber_stars.unwrap_or(max_amber).min(max_amber));

                // Apply sculpture stars in crate order: (amber, cyan)
                if let (Some(c), Some(a)) = (cyan_stars, amber_stars)
                  // Only send non-zero star counts
                  && (c > 0 || a > 0)
                {
                    order = order.with_sculpture_stars(a, c);
                }

                if let Some(pt) = per_trade {
                    order = order.with_per_trade(pt);
                }

                let mut _active_slots_count = 0;

                match wfm_client.create_order(order).await {
                    Ok(_) => {
                        println!("\x1B[32m[SYNC] Successfully listed {} x{}!\x1B[0m", item.name, quantity);
                        session_report_items.push(SessionReportItem {
                            name: item.name.clone(),
                            slug: item.slug.clone(),
                            price,
                            quantity,
                            rank: rank_opt.map(u32::from),
                            action: "Created".to_string(),
                        });
                        active_slots_count += 1;
                    }
                    Err(e) => {
                        eprintln!("\x1B[31m[SYNC_ERROR] Failed to list {}: {}\x1B[0m", item.name, e);
                    }
                }
            }
        }
    }

    // 8. Session Report Persistence
    let final_report = SessionReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        username,
        items_processed: session_report_items,
    };

    let report_content = serde_json::to_string_pretty(&final_report)?;
    fs::write("session_report.json", report_content)?;
    print_header("WFM Pricer Session Completed Successfully");
    println!("Session report written to: 'session_report.json'");

    Ok(())
}

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
    // 1. Exact slug + exact rank
    if let Some(rules) = keeplist.items.get(slug) {
        if let Some(rank_val) = rank
            && let Some(rule) = rules.iter().find(|r| r.rank == Some(rank_val))
        {
            return rule.keep;
        }
        // 2. Slug default (rank = None)
        if let Some(rule) = rules.iter().find(|r| r.rank.is_none()) {
            return rule.keep;
        }
    }
    // 3. Category default
    if let Some(rule) = keeplist.defaults.get(category) {
        return rule.keep;
    }
    // 4. Sell everything
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
