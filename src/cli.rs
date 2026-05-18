use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use crate::models::{MappedItem, KeepEntry, BlacklistEntry};
use crate::client::{WfmClient, UserListing};
use crate::pricing::{
    fetch_statistics, calculate_weighted_average, calculate_saturation_ratio,
    derive_endo_to_plat_rate, get_ayatan_endo_yield
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

enum ListingTask {
    Create {
        item_id: String,
        price: u32,
        quantity: u32,
        rank: Option<u32>,
        name: String,
        slug: String,
    },
    Update {
        order_id: String,
        price: u32,
        quantity: u32,
        name: String,
        slug: String,
    },
}

/// The CLI UI dashboard printer. Uses ANSI colors for high visual premium appeal.
fn print_header(title: &str) {
    println!("\x1B[1;36m================================================================================\x1B[0m");
    println!("\x1B[1;35m   {}   \x1B[0m", title.to_uppercase());
    println!("\x1B[1;36m================================================================================\x1B[0m");
}

fn print_info(label: &str, value: &str) {
    println!("\x1B[1;34m  {:<25}\x1B[0m : \x1B[32m{}\x1B[0m", label, value);
}

fn print_warning(msg: &str) {
    println!("\x1B[1;33m  [WARNING] {}\x1B[0m", msg);
}

#[allow(dead_code)]
fn print_error_ui(msg: &str) {
    println!("\x1B[1;31m  [ERROR] {}\x1B[0m", msg);
}

/// Runs the interactive CLI loop.
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
    let mut wfm_client = WfmClient::new();
    wfm_client.sign_in(&email, &password).await?;
    let username = wfm_client.user.as_ref().unwrap().ingame_name.clone();

    // 3. Retrieve user listings and track budget slot limit (100)
    println!("Fetching your active listings from Warframe.Market...");
    let user_listings = wfm_client.get_sell_listings(&username).await?;
    let current_listing_count = user_listings.len();
    print_info("Active Listings on WFM", &format!("{}/100 slots used", current_listing_count));

    // Map existing listings by url_name for quick lookup/budget calculation
    let mut existing_listings_map: HashMap<String, Vec<UserListing>> = HashMap::new();
    for listing in user_listings {
        existing_listings_map.entry(listing.item.url_name.clone()).or_default().push(listing);
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
                if let Some(mr) = item.max_rank {
                    if mr >= 5 {
                        return true;
                    }
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
    let mut priced_ayatans = HashMap::new();
    for c in &candidates {
        if c.is_ayatan {
            if let Ok(stats) = fetch_statistics(&c.slug).await {
                let (wa_price, _) = calculate_weighted_average(&stats, None);
                if wa_price > 0.0 {
                    priced_ayatans.insert(c.slug.clone(), wa_price);
                }
            }
        }
    }

    let endo_rate = derive_endo_to_plat_rate(&priced_ayatans);
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
            let est_plat = (yield_endo as f64) * endo_rate;
            if let Ok(stats) = fetch_statistics(slug).await {
                // Find latest buy orders from statistics_live
                let avg_buy = stats.payload.statistics_live.ninety_days
                    .iter()
                    .filter(|d| d.order_type.as_deref() == Some("buy"))
                    .last()
                    .map(|d| d.wa_price)
                    .unwrap_or(0.0);

                let arbitrage = if avg_buy > 0.0 && avg_buy < est_plat - 1.0 {
                    "\x1B[1;32mYES\x1B[0m"
                } else {
                    "\x1B[31mNO\x1B[0m"
                };

                println!("  {:<30} | {:<12} | {:<10.2} | {:<12.2} | {:<10}", slug, yield_endo, est_plat, avg_buy, arbitrage);
            }
        }
    }
    println!();

    // 6. Async channel listing tasks with backpressure
    let (tx, mut rx) = mpsc::channel::<ListingTask>(10);

    // Spawn the background listing queue processor
    // Shares the WFM client logic synchronously using rate limits (1 req/400ms)
    let wfm_client_worker = wfm_client; // transfer ownership to the background worker
    
    // Track successfully listed items to output in our session report
    let report_items_arc = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let report_items_clone = report_items_arc.clone();

    let bg_handle = tokio::spawn(async move {
        while let Some(task) = rx.recv().await {
            // Rate limit: Sleep 400ms before each listing action
            sleep(Duration::from_millis(400)).await;

            match task {
                ListingTask::Create { item_id, price, quantity, rank, name, slug } => {
                    println!("\x1B[33m[SYNC] Posting listing: {} (rank: {:?}) for {} plat...\x1B[0m", name, rank, price);
                    match wfm_client_worker.create_listing(&item_id, price, quantity, rank).await {
                        Ok(_) => {
                            println!("\x1B[32m[SYNC] Successfully listed {} x{}!\x1B[0m", name, quantity);
                            let mut list = report_items_clone.lock().unwrap();
                            list.push(SessionReportItem {
                                name,
                                slug,
                                price,
                                quantity,
                                rank,
                                action: "Created".to_string(),
                            });
                        }
                        Err(e) => {
                            eprintln!("\x1B[31m[SYNC_ERROR] Failed to list {}: {}\x1B[0m", name, e);
                        }
                    }
                }
                ListingTask::Update { order_id, price, quantity, name, slug } => {
                    println!("\x1B[33m[SYNC] Updating listing: {} to {} plat...\x1B[0m", name, price);
                    match wfm_client_worker.update_listing(&order_id, price, quantity).await {
                        Ok(_) => {
                            println!("\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m", name);
                            let mut list = report_items_clone.lock().unwrap();
                            list.push(SessionReportItem {
                                name,
                                slug,
                                price,
                                quantity,
                                rank: None,
                                action: "Updated".to_string(),
                            });
                        }
                        Err(e) => {
                            eprintln!("\x1B[31m[SYNC_ERROR] Failed to update listing {}: {}\x1B[0m", order_id, e);
                        }
                    }
                }
            }
        }
    });

    // 7. Interactive loop and candidate evaluation
    print_header("Trade Candidate Evaluation");
    println!("Fetching WFM pricing and volume stats dynamically for candidates...");

    let mut priced_candidates = Vec::new();
    let mut active_slots_count = current_listing_count;

    for c in candidates {
        // Fetch stats dynamically
        if let Ok(stats) = fetch_statistics(&c.slug).await {
            // If mod or arcane, statistics has rank-specific entries. 
            // We match the candidate's rank. If it's a prime part, rank is None (0).
            let target_rank = if c.is_mod || c.is_arcane { Some(c.rank) } else { None };
            let (wa_price, _vol_90d) = calculate_weighted_average(&stats, target_rank);
            let saturation = calculate_saturation_ratio(&stats, target_rank);

            // Compute score: wa_price * log(1 + volume_30d)
            // As a proxy for volume_30d, we'll extract the volume from last 30 daily stats items in closed payload
            let ninety_days_closed = &stats.payload.statistics_closed.ninety_days;
            let vol_30d: u32 = ninety_days_closed.iter()
                .filter(|d| d.mod_rank == target_rank)
                .take(30)
                .map(|d| d.volume)
                .sum();

            let score = wa_price * (1.0 + (vol_30d as f64)).ln();

            priced_candidates.push((c, wa_price, saturation, vol_30d, score));
        }
    }

    // Sort descending by score
    // Items already listed are prioritized first as update candidates (per interactive-cli spec)
    priced_candidates.sort_by(|a, b| {
        let a_listed = existing_listings_map.contains_key(&a.0.slug);
        let b_listed = existing_listings_map.contains_key(&b.0.slug);
        if a_listed && !b_listed {
            std::cmp::Ordering::Less
        } else if !a_listed && b_listed {
            std::cmp::Ordering::Greater
        } else {
            b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    // Load blacklist/keeplist local caches for dynamically editing inside loop
    let mut blacklist_set = load_blacklist()?;

    let mut stdout = io::stdout();

    for (mut item, wa_price, saturation, vol_30d, _score) in priced_candidates {
        if blacklist_set.contains(&item.slug) {
            continue; // Skip blacklisted slug dynamically
        }

        // Apply keeplist check dynamically
        let keep_copies = get_keep_quantity(&item.slug, item.rank)?;
        if keep_copies > 0 {
            if item.quantity <= keep_copies {
                continue; // Skipped entirely since all copies are kept
            }
            item.quantity -= keep_copies;
        }

        // Check budget limits
        let is_already_listed = existing_listings_map.contains_key(&item.slug);
        if active_slots_count >= 100 && !is_already_listed {
            print_warning(&format!("Budget limit reached (100/100 slots). Skipping listing creation candidate: {}", item.name));
            continue;
        }

        println!("\x1B[1;36m--------------------------------------------------------------------------------\x1B[0m");
        println!("\x1B[1mCANDIDATE\x1B[0m: \x1B[1;32m{}\x1B[0m | Slug: {} | Qty Available: {}", item.name, item.slug, item.quantity);
        println!("  Rank: {:<5} | 30d Vol: {:<6} | Est Price (WA): \x1B[1;33m{:.1} plat\x1B[0m", item.rank, vol_30d, wa_price);
        println!("  Saturation Ratio: {:.3} (sell volume vs closed volume)", saturation);
        println!("  Already Listed on WFM: {}", if is_already_listed { "\x1B[1;32mYES\x1B[0m" } else { "\x1B[31mNO\x1B[0m" });

        if is_already_listed {
            if let Some(listings) = existing_listings_map.get(&item.slug) {
                for (idx, listing) in listings.iter().enumerate() {
                    println!("    [{}] Listed price: {} plat | Qty listed: {} | Visible: {}", idx + 1, listing.price, listing.quantity, listing.visible);
                }
            }
        }

        // Backpressure queue depth check
        // If queue exceeds 5 items, block prompt
        loop {
            // A primitive check by using the channel capacity or just direct sync delay:
            // Since we use a channel of capacity 10, backpressure blocks organically if full.
            // But we can let the prompt sleep if we want to give the worker a headstart:
            break;
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
            blacklist_set.insert(item.slug.clone());
            save_blacklist(&blacklist_set)?;
            continue;
        } else if choice == "K" {
            print!("\x1B[1;34m  How many copies of {} (rank {}) do you want to keep? \x1B[0m", item.name, item.rank);
            let _ = stdout.flush();
            let mut keep_str = String::new();
            io::stdin().read_line(&mut keep_str)?;
            if let Ok(keep_qty) = keep_str.trim().parse::<u32>() {
                add_to_keeplist(&item.slug, item.rank, keep_qty)?;
                println!("Saved to keeplist.json!");
            }
            continue;
        } else if choice == "Y" {
            // Prompt price
            print!("  Price to list (default {:.1}): ", wa_price);
            let _ = stdout.flush();
            let mut price_str = String::new();
            io::stdin().read_line(&mut price_str)?;
            let price: u32 = price_str.trim().parse::<u32>().unwrap_or(wa_price.round() as u32);

            // Prompt quantity
            print!("  Quantity to list (default {}): ", item.quantity);
            let _ = stdout.flush();
            let mut qty_str = String::new();
            io::stdin().read_line(&mut qty_str)?;
            let quantity: u32 = qty_str.trim().parse::<u32>().unwrap_or(item.quantity);

            if is_already_listed {
                // Update listing
                if let Some(listings) = existing_listings_map.get(&item.slug) {
                    if let Some(first_listing) = listings.first() {
                        let _ = tx.send(ListingTask::Update {
                            order_id: first_listing.id.clone(),
                            price,
                            quantity,
                            name: item.name.clone(),
                            slug: item.slug.clone(),
                        }).await;
                    }
                }
            } else {
                // Create listing
                let rank_opt = if item.is_mod || item.is_arcane { Some(item.rank) } else { None };
                let _ = tx.send(ListingTask::Create {
                    item_id: item.id.clone(),
                    price,
                    quantity,
                    rank: rank_opt,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                }).await;
                active_slots_count += 1;
            }
        }
    }

    // Drop the sender to signal worker to finish
    drop(tx);
    let _ = bg_handle.await;

    // 8. Session Report Persistence
    let final_report = SessionReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        username,
        items_processed: report_items_arc.lock().unwrap().clone(),
    };

    let report_content = serde_json::to_string_pretty(&final_report)?;
    fs::write("session_report.json", report_content)?;
    print_header("WFM Pricer Session Completed Successfully");
    println!("Session report written to: 'session_report.json'");

    Ok(())
}

fn load_blacklist() -> Result<HashSet<String>, Box<dyn Error + Send + Sync>> {
    if Path::new("blacklist.json").exists() {
        let raw = fs::read_to_string("blacklist.json")?;
        let entries: Vec<BlacklistEntry> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(entries.into_iter().map(|e| e.slug).collect())
    } else {
        Ok(HashSet::new())
    }
}

fn save_blacklist(set: &HashSet<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let entries: Vec<BlacklistEntry> = set.iter().map(|s| BlacklistEntry { slug: s.clone() }).collect();
    let raw = serde_json::to_string_pretty(&entries)?;
    fs::write("blacklist.json", raw)?;
    Ok(())
}

fn get_keep_quantity(slug: &str, rank: u32) -> Result<u32, Box<dyn Error + Send + Sync>> {
    if Path::new("keeplist.json").exists() {
        let raw = fs::read_to_string("keeplist.json")?;
        let entries: Vec<KeepEntry> = serde_json::from_str(&raw).unwrap_or_default();
        for entry in entries {
            if entry.slug == slug && entry.rank == rank {
                return Ok(entry.keep);
            }
        }
    }
    Ok(0)
}

fn add_to_keeplist(slug: &str, rank: u32, qty: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut entries: Vec<KeepEntry> = if Path::new("keeplist.json").exists() {
        let raw = fs::read_to_string("keeplist.json")?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        Vec::new()
    };

    // Update if exists, otherwise add
    let mut found = false;
    for entry in &mut entries {
        if entry.slug == slug && entry.rank == rank {
            entry.keep = qty;
            found = true;
            break;
        }
    }

    if !found {
        entries.push(KeepEntry {
            slug: slug.to_string(),
            rank,
            keep: qty,
        });
    }

    let raw = serde_json::to_string_pretty(&entries)?;
    fs::write("keeplist.json", raw)?;
    Ok(())
}
