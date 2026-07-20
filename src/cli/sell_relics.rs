use std::collections::HashMap;
use crate::AppResult;
use tokio::time::{Duration, sleep};

use super::{capitalize_tier, mapping, print_header, tseprintln, tsprintln, wfm_client};

/// One ready-to-send whisper, already matched against a live buy order this account can
/// actually fulfill (enough owned copies of that exact relic/tier to cover the order's lot
/// size).
pub(crate) struct RelicSellMessage {
    ign: String,
    display_name: String,
    tier: String,
    /// Total platinum for the trade (i.e. what you'd actually receive for `lot_size` relics
    /// in one go) — this is what gets quoted in the whisper.
    price: u32,
    /// perTrade lot size for the matched order. WFM's `platinum` is the price for the whole
    /// lot, not per relic — a `platinum: 40, perTrade: 6` order is ~6.67p/relic, not 40p/relic.
    /// Kept alongside `price` so the message can show "6x [...]" for batch orders.
    lot_size: u32,
    /// The order's own `quantity` field — how many total the buyer is looking to acquire
    /// across (potentially many) trades, NOT the lot size of any single trade. Shown above the
    /// whisper purely as context (e.g. "they want 36 total, you have 12") — never filtered on,
    /// since fulfillability only depends on `lot_size`.
    listing_quantity: u32,
    /// Total owned of this exact relic/tier at the time of the check, shown alongside
    /// `listing_quantity` for the same reason.
    owned_quantity: u32,
}

/// `sell-relics`: matches owned relics (by refinement/tier) against the live public buy-order
/// book and prints a whisper message for the best-paying, fulfillable buy order on each one,
/// sorted descending by per-relic rate (platinum ÷ perTrade lot size — see the `unit_price`
/// comment below for why raw lot totals would rank batch orders wrong).
///
/// Important WFM-specific detail, confirmed against the real API: unlike most bulk-tradable
/// items, a relic's WFM slug is NOT refinement-specific — `lith_o2_relic` alone covers Intact
/// through Radiant. Refinement is carried as each individual order's `subtype` field instead
/// (see `PublicOrder::subtype`), so this fetches each distinct relic's order book exactly
/// once and then splits by `subtype` locally, rather than issuing one fetch per tier.
///
/// Unlike `run_check_sets_cli`/the default pipeline, this never logs into WFM — whispering a
/// buyer only needs their in-game name off the *public* per-item order book
/// (`wfm_client::fetch_item_orders`), not an authenticated `WfmClient`.
///
/// # Errors
/// Returns an error if caches can't be refreshed, the inventory file can't be found/decrypted,
/// or inventory mapping fails outright. Per-item order-fetch failures are logged and skipped
/// rather than aborting the whole run, matching `run_check_sets_cli`'s behavior.
#[allow(clippy::too_many_lines)]
pub async fn run_sell_relics_cli(min_price: Option<u32>) -> AppResult<()> {
    print_header("Sell Relics — Matching Buy Orders");

    mapping::update_caches().await?;

    tsprintln!("Ingesting inventory...");
    let inventory_path = crate::resolve_inventory_path(None)?;
    let inventory = crate::ingestion::ingest_inventory(&inventory_path)?;

    let client = reqwest::Client::new();
    let mapped_items = mapping::map_inventory(&inventory, &client).await?;

    // Owned quantity per (base slug, refinement) — NOT just per slug, since one slug now
    // covers all four tiers (see doc comment above). Aggregated rather than trusting one
    // MappedItem per combination, since nothing in map_inventory guarantees inventory.json
    // can't list the same game_ref across more than one array entry; understating owned
    // quantity here would just mean skipping a fulfillable sale, but overstating it would
    // mean whispering a buyer we can't actually deliver to.
    let mut relic_qty: HashMap<(String, String), u32> = HashMap::new();
    let mut relic_names: HashMap<String, String> = HashMap::new(); // slug -> base display name

    for item in mapped_items
        .iter()
        .filter(|i| i.category() == "relic" && i.quantity > 0)
    {
        let Some(tier) = &item.owned_subtype else {
            // Should be unreachable — mapping::process_relic always sets owned_subtype for
            // anything it produces, and category() == "relic" only matches slugs containing
            // "_relic", which process_relic is the sole source of. Logged rather than silently
            // dropped in case that invariant ever breaks upstream.
            tseprintln!(
                "[WARNING] '{}' ({}) matched the relic category but has no recorded refinement — skipping.",
                item.name,
                item.slug
            );
            continue;
        };
        *relic_qty
            .entry((item.slug.clone(), tier.clone()))
            .or_insert(0) += item.quantity;
        relic_names
            .entry(item.slug.clone())
            .or_insert_with(|| item.name.clone());
    }

    if relic_qty.is_empty() {
        tsprintln!("No relics found in inventory.");
        return Ok(());
    }

    // Group owned tiers by slug so each distinct relic gets exactly one order-book fetch —
    // a single `/v2/orders/item/{slug}` call returns every refinement's orders at once.
    let mut owned_tiers_by_slug: HashMap<String, Vec<String>> = HashMap::new();
    for (slug, tier) in relic_qty.keys() {
        owned_tiers_by_slug
            .entry(slug.clone())
            .or_default()
            .push(tier.clone());
    }

    tsprintln!(
        "Checking live buy orders for {} owned relic(s) across {} relic/tier combination(s) (respects WFM's 3 req/s limit, so this may take a bit)...\n",
        owned_tiers_by_slug.len(),
        relic_qty.len()
    );

    let mut messages: Vec<RelicSellMessage> = Vec::new();
    let mut no_buy_orders = 0u32;
    let mut no_fulfillable_order = 0u32;

    // Deterministic fetch order; HashMap iteration order isn't otherwise meaningful since
    // results get sorted by price before display anyway.
    let mut slugs: Vec<&String> = owned_tiers_by_slug.keys().collect();
    slugs.sort();

    for slug in slugs {
        let display_name = relic_names
            .get(slug)
            .cloned()
            .unwrap_or_else(|| slug.clone());

        let orders = match wfm_client::fetch_item_orders(&client, slug).await {
            Ok(o) => o,
            Err(e) => {
                tseprintln!("Failed to fetch orders for {display_name}: {e}");
                continue;
            }
        };
        sleep(Duration::from_millis(350)).await;

        let mut tiers = owned_tiers_by_slug[slug].clone();
        tiers.sort();
        tiers.dedup();

        for tier in tiers {
            let owned_qty = relic_qty
                .get(&(slug.clone(), tier.clone()))
                .copied()
                .unwrap_or(0);

            let buy_orders_for_tier = || {
                orders.iter().filter(|o| {
                    o.is_buy() && o.visible && o.subtype.as_deref() == Some(tier.as_str())
                })
            };

            // Only buy orders whose lot size (perTrade) we can actually cover with what's in
            // inventory count as fulfillable — a buy order asking for a bigger lot than we own
            // of this exact relic/tier can't be delivered on, regardless of price.
            let mut candidates: Vec<&crate::wfm_client::PublicOrder> = buy_orders_for_tier()
                .filter(|o| o.lot_size() <= owned_qty)
                .collect();

            // WFM's `platinum` is the price for the whole lot (perTrade relics), not per
            // relic — a 6-relic lot at platinum: 40 is ~6.67p/relic, not 40p/relic. Both
            // `--min-price` and "best" order selection need to compare on that per-relic rate,
            // or a large low-value lot can look like the best (or only qualifying) offer when
            // it's actually the worst.
            let unit_price = |o: &crate::wfm_client::PublicOrder| {
                f64::from(o.platinum) / f64::from(o.lot_size())
            };

            if let Some(min) = min_price {
                candidates.retain(|o| unit_price(o) >= f64::from(min));
            }

            let Some(best) = candidates
                .iter()
                .max_by(|a, b| unit_price(a).total_cmp(&unit_price(b)))
            else {
                if buy_orders_for_tier().next().is_some() {
                    no_fulfillable_order += 1;
                } else {
                    no_buy_orders += 1;
                }
                continue;
            };

            let Some(user) = &best.user else {
                tseprintln!(
                    "[WARNING] Best buy order for {display_name} - {} at {}p has no attached trader info — skipping.",
                    capitalize_tier(&tier),
                    best.platinum
                );
                continue;
            };

            messages.push(RelicSellMessage {
                ign: user.ingame_name.clone(),
                display_name: display_name.clone(),
                tier: capitalize_tier(&tier),
                price: best.platinum,
                lot_size: best.lot_size(),
                listing_quantity: best.quantity,
                owned_quantity: owned_qty,
            });
        }
    }

    // Sort by per-relic rate (platinum / lot_size), not raw lot total — see the unit_price
    // comment above for why. A 6-relic lot at 40p total (~6.67p/relic) should rank below a
    // single-relic order at 10p, not above it.
    messages.sort_by(|a, b| {
        let rate = |m: &RelicSellMessage| f64::from(m.price) / f64::from(m.lot_size);
        rate(b).total_cmp(&rate(a))
    });

    print_header(&format!("Whisper Messages ({})", messages.len()));
    if messages.is_empty() {
        tsprintln!(
            "(No owned relic had a fulfillable buy order{}.)",
            if min_price.is_some() {
                " at or above the requested minimum price"
            } else {
                ""
            }
        );
    }
    for m in &messages {
        tsprintln!(
            "{} - {}  (listing wants {}, you own {})",
            m.display_name,
            m.tier,
            m.listing_quantity,
            m.owned_quantity
        );
        // Unescaped: [Name] renders as an in-game item link (helps with localization) and
        // :platinum: as the in-game emote when pasted into WFM/Warframe chat directly.
        let qty_prefix = if m.lot_size > 1 {
            format!("{}x ", m.lot_size)
        } else {
            String::new()
        };
        tsprintln!(
            "/w {} Hi! I want to sell: {qty_prefix}[{}] - {} for {}:platinum: (warframe.market)\n",
            m.ign,
            m.display_name,
            m.tier,
            m.price
        );
    }

    if no_buy_orders > 0 || no_fulfillable_order > 0 {
        tsprintln!(
            "\n({no_buy_orders} relic/tier combo(s) had no visible buy orders at all; {no_fulfillable_order} had buy orders but none we could fulfill at the requested lot size/min price.)"
        );
    }

    Ok(())
}
