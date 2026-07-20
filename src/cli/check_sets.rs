use crate::{AppResult, http};
use tokio::time::{Duration, sleep};

use super::{mapping, print_header, resolve_set_item, tseprintln, tsprintln, wfm_client};

pub(crate) struct PricedIncompleteSet {
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
#[allow(clippy::too_many_lines)]
pub async fn run_check_sets_cli(min_profit: Option<f64>) -> AppResult<()> {
    print_header("Incomplete Set Profitability Check");

    mapping::update_caches().await?;

    tsprintln!("Ingesting inventory...");
    let inventory_path = crate::resolve_inventory_path(None)?;
    let inventory = crate::ingestion::ingest_inventory(&inventory_path)?;

    let client = http::shared_client();
    let mapped_items = mapping::map_inventory(&inventory, client).await?;
    let (_parent_map, requirements) = mapping::load_build_maps()?;
    let (wfcd_by_ref, wfm_by_ref, wfm_by_name, _wfm_by_slug) = mapping::load_lookup_tables()?;

    let incomplete = mapping::find_incomplete_sets(
        &mapped_items,
        &requirements,
        &wfcd_by_ref,
        &wfm_by_ref,
        &wfm_by_name,
    );
    if incomplete.is_empty() {
        tsprintln!(
            "No incomplete Sets found — every Set you own parts of is either complete already or you don't own any of its parts yet."
        );
        return Ok(());
    }

    // Report everything the completeness check found, unconditionally, before touching the
    // network. This is the audit trail for `find_incomplete_sets` itself — if something looks
    // wrong here (a Set that shouldn't be sellable, a missing-part quantity that looks off,
    // etc.), that's a detection-logic bug, independent of anything the pricing pass below does.
    print_header(&format!("Incomplete Sets Found ({})", incomplete.len()));
    for set in &incomplete {
        let name = wfcd_by_ref
            .get(&set.build_unique)
            .map_or(set.build_unique.as_str(), |w| w.name.as_str());
        let parts: Vec<String> = set
            .missing
            .iter()
            .map(|c| format!("{}x {}", c.deficit, c.name))
            .collect();
        tsprintln!("  {name}: needs {}", parts.join(", "));
    }
    tsprintln!(
        "\nChecking current buy/sell orders for each (respects WFM's 3 req/s limit, so this may take a bit)...\n"
    );

    let mut priced = Vec::new();

    for set in &incomplete {
        let Some(wfcd_item) = wfcd_by_ref.get(&set.build_unique) else {
            continue;
        };
        // find_incomplete_sets already only returns builds with a resolvable WFM Set
        // listing, so this should always succeed — treated as a hard skip (not silently
        // dropped: it still shows up in the "Incomplete Sets Found" list above) if the two
        // ever disagree.
        let Some(set_wfm_item) = resolve_set_item(&wfcd_item.name, &wfm_by_name) else {
            tseprintln!(
                "[WARNING] '{}' passed completeness detection but has no resolvable WFM Set listing — this is a bug, please report it.",
                wfcd_item.name
            );
            continue;
        };

        let mut unpriced_reason: Option<String> = None;

        let set_sell_price = match wfm_client::fetch_item_orders(client, &set_wfm_item.slug).await
        {
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

            let orders = match wfm_client::fetch_item_orders(client, &wfm_comp.slug).await {
                Ok(o) => o,
                Err(e) => {
                    unpriced_reason =
                        Some(format!("failed to fetch orders for '{}': {e}", comp.name));
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
                None => {
                    if let Some(price) = wfm_client::best_sell_price(&orders, None) {
                        (price, true)
                    } else {
                        unpriced_reason = Some(format!(
                            "no current buy or sell orders for missing part '{}'",
                            comp.name
                        ));
                        break;
                    }
                }
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

    priced.sort_by(|a, b| {
        b.profit
            .partial_cmp(&a.profit)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    print_header("Set Completion Profitability");
    tsprintln!(
        "{:<32} {:>12} {:>15} {:>10}",
        "Set",
        "Parts Cost",
        "Set Sell Price",
        "Profit"
    );
    tsprintln!("{}", "-".repeat(73));

    let mut shown = 0;
    for set in &priced {
        if let Some(min) = min_profit
            && set.profit < min
        {
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
                tsprintln!(
                    "    need {deficit}x {part_name} @ {unit_price}p* (no buy orders — priced off current best sell order instead)"
                );
            } else {
                tsprintln!(
                    "    need {deficit}x {part_name} @ {unit_price}p (current best buy order)"
                );
            }
        }
    }

    if shown == 0 {
        tsprintln!("(No priced Sets met the requested minimum profit.)");
    }

    Ok(())
}
