use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{self, Write};
use tokio::time::{Duration, sleep};

use super::{
    BlacklistConfig, BuildParentMap, CandidateContext, CreateOrder, KeepConfig, ListingKey,
    MappedItem, NoOpDecision, OwnedOrder, SessionReportItem, UpdateOrder, WfmClient,
    add_to_keeplist, ayatan_max_stars, decide_no_op, find_same_price_order, get_auto_keep,
    get_ayatan_endo_yield, get_keep_quantity, print_warning, quantity_default,
    resolve_action_choice, resolve_keep_copies, save_blacklist, tseprintln, tsprint, tsprintln,
};

// This function is a single linear decision pipeline (keep/blacklist checks, price-vs-Endo
// comparison, listing-state lookup, no-op/quantity-sync short-circuit, then the interactive
// prompt). Splitting it purely to satisfy the line-count lint would mean threading the same
// half-dozen pieces of state through several smaller functions for no behavioral benefit;
// left as-is deliberately rather than fixed blindly without a compiler to verify a refactor.
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_single_candidate(
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
        if item.quantity <= keep_copies {
            return Ok(None);
        }
        item.quantity -= keep_copies;
    }

    if item.is_ayatan
        && let Some(endo_yield) = get_ayatan_endo_yield(&item.slug)
    {
        let endo_value = f64::from(endo_yield) * ctx.endo_rate;
        if wa_price < endo_value * 1.15 {
            tsprintln!(
                "[SKIP] {} worth {:.1}p as Endo (vs {:.1}p market)",
                item.name,
                endo_value,
                wa_price
            );
            return Ok(None);
        }
    }

    let listing_key = ListingKey {
        item_id: item.id.clone(),
        rank: if item.is_mod || item.is_arcane {
            item.rank
        } else {
            None
        },
    };

    let matching_listings = ctx.existing_listings_map.get(&listing_key);
    let listed_qty: u32 = matching_listings.map_or(0, |listings| {
        listings
            .iter()
            .map(crate::wfm_client::Order::quantity)
            .sum()
    });

    let available_qty = item.quantity.saturating_sub(listed_qty);
    let is_already_listed = matching_listings.is_some();

    if available_qty == 0 {
        return Ok(None);
    }

    if *ctx.active_slots_count >= 100 && !is_already_listed {
        print_warning(&format!(
            "Budget limit reached (100/100 slots). Skipping listing creation candidate: {}",
            item.name
        ));
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
                NoOpDecision::QuantitySyncOnly {
                    new_quantity,
                    keep_price,
                } => {
                    // Silently update the listing with the new quantity, keeping the price exactly as-is.
                    let update = UpdateOrder::new()
                        .platinum(keep_price)
                        .quantity(new_quantity);
                    if let Err(e) = ctx.wfm_client.update_order(existing.id(), update).await {
                        tseprintln!(
                            "\x1B[31m[SYNC_ERROR] Failed to sync quantity for {}: {}\x1B[0m",
                            item.name,
                            e
                        );
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

    tsprintln!(
        "\x1B[1;36m--------------------------------------------------------------------------------\x1B[0m"
    );
    tsprintln!(
        "\x1B[1mCANDIDATE\x1B[0m: \x1B[1;32m{}\x1B[0m | Slug: {} | Qty Available: {}",
        item.name,
        item.slug,
        available_qty
    );
    tsprintln!(
        "  Rank: {:<5} | 30d Vol: {:<6} | Est Price (WA): \x1B[1;33m{:.1} plat\x1B[0m",
        item.rank.unwrap_or(0),
        vol_30d,
        wa_price
    );
    tsprintln!("  Saturation Ratio: {saturation:.3} (sell volume vs closed volume)");
    tsprintln!(
        "  Already Listed on WFM: {}",
        if is_already_listed {
            "\x1B[1;32mYES\x1B[0m"
        } else {
            "\x1B[31mNO\x1B[0m"
        }
    );

    if is_already_listed && let Some(listings) = ctx.existing_listings_map.get(&listing_key) {
        for (idx, listing) in listings.iter().enumerate() {
            tsprintln!(
                "    [{}] Listed price: {} plat | Qty listed: {} | Visible: {}",
                idx + 1,
                listing.platinum(),
                listing.quantity(),
                listing.is_visible()
            );
        }
    }

    tsprint!(
        "\x1B[1;35m  Action? [Enter/Y] List/Update | [N] Skip | [K] Add to Keep List | [B] Blacklist | [X] Save & Exit: \x1B[0m"
    );
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
        tsprint!(
            "\x1B[1;34m  How many copies of {} (rank {}) do you want to keep? \x1B[0m",
            item.name,
            item.rank.unwrap_or(0)
        );
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
        )
        .await;
    }

    Ok(None)
}

// Same rationale as handle_single_candidate above: this is a single interactive prompt-then-act
// sequence (price prompt, quantity prompt, price-conflict detection, sculpture/per-trade prompts,
// create-or-update). Deliberately left as one function rather than split blind.
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_list_or_update(
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
    let existing_same_price_order =
        find_same_price_order(ctx.existing_listings_map, &item.id, listing_key.rank, price);

    if let Some(order) = existing_same_price_order {
        tsprintln!(
            "\x1B[33m[SYNC] Found an existing order for {} at the same price ({} plat). Updating its quantity to {}...\x1B[0m",
            item.name,
            price,
            quantity
        );
        let update = UpdateOrder::new().platinum(price).quantity(quantity);
        match ctx.wfm_client.update_order(order.id(), update).await {
            Ok(()) => {
                tsprintln!(
                    "\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m",
                    item.name
                );
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
    if item.is_ayatan && item.slug.ends_with("_sculpture") {
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

    // ── Handle update or create ──────────────────────────────────────────────
    if is_already_listed {
        if let Some(listings) = ctx.existing_listings_map.get(listing_key)
            && let Some(first_listing) = listings.first()
        {
            tsprintln!(
                "\x1B[33m[SYNC] Updating listing: {} to {} plat...\x1B[0m",
                item.name,
                price
            );
            sleep(Duration::from_millis(400)).await;
            let update = UpdateOrder::new().platinum(price).quantity(quantity);
            match ctx
                .wfm_client
                .update_order(first_listing.id(), update)
                .await
            {
                Ok(()) => {
                    tsprintln!(
                        "\x1B[32m[SYNC] Successfully updated listing for {}!\x1B[0m",
                        item.name
                    );
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
                    tseprintln!(
                        "\x1B[31m[SYNC_ERROR] Failed to update listing {}: {}\x1B[0m",
                        first_listing.id(),
                        e
                    );
                    return Ok(None);
                }
            }
        }
    } else {
        let rank_opt = if item.is_mod || item.is_arcane {
            item.rank
        } else {
            None
        };
        tsprintln!(
            "\x1B[33m[SYNC] Posting listing: {} (rank: {:?}) for {} plat...\x1B[0m",
            item.name,
            rank_opt,
            price
        );
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
                tsprintln!(
                    "\x1B[32m[SYNC] Successfully listed {} x{}!\x1B[0m",
                    item.name,
                    quantity
                );
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
                tseprintln!(
                    "\x1B[31m[SYNC_ERROR] Failed to list {}: {}\x1B[0m",
                    item.name,
                    e
                );
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
pub(crate) async fn process_candidates(
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
        match handle_single_candidate(item, wa_price, saturation, vol_30d, &mut ctx).await {
            Ok(Some(report_item)) => session_items.push(report_item),
            Ok(None) => {}
            Err(e) if e.to_string() == "EXIT_REQUESTED" => break,
            Err(e) => return Err(e),
        }
    }

    Ok(session_items)
}
