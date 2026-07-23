//! `RelicSellService` — everything `--sell-relics` needs before it can print whisper
//! messages: refresh caches, ingest/map inventory, aggregate owned relics by
//! (slug, refinement tier), fetch each relic's live order book, and match owned
//! quantity against the best-paying fulfillable buy order.
//!
//! Extracted from `cli::sell_relics::run_sell_relics_cli` (Architecture Evolution Plan,
//! Phase 1.5), following the same split `SetAnalysisService` established: this service
//! does the network calls and matching and returns plain structs; `cli::sell_relics`
//! stays limited to printing them. `best_fulfillable_relic_order` and its unit tests
//! moved here too, since it's decision logic, not presentation, and belongs alongside
//! the workflow that calls it.

use std::collections::HashMap;
use tokio::time::{Duration, sleep};

use crate::wfm_client::PublicOrder;
use crate::{AppResult, http, ingestion, mapping, tseprintln, wfm_client};

/// One ready-to-send whisper, already matched against a live buy order this account can
/// actually fulfill (enough owned copies of that exact relic/tier to cover the order's lot
/// size). `tier` is the raw lowercase refinement string (e.g. `"radiant"`) — display
/// capitalization is a presentation concern and stays in `cli::sell_relics`.
pub(crate) struct RelicSellMessage {
    pub(crate) ign: String,
    pub(crate) display_name: String,
    pub(crate) tier: String,
    /// Total platinum for the trade (i.e. what you'd actually receive for `lot_size` relics
    /// in one go) — this is what gets quoted in the whisper.
    pub(crate) price: u32,
    /// perTrade lot size for the matched order. WFM's `platinum` is the price for the whole
    /// lot, not per relic — a `platinum: 40, perTrade: 6` order is ~6.67p/relic, not 40p/relic.
    /// Kept alongside `price` so the message can show "6x [...]" for batch orders.
    pub(crate) lot_size: u32,
    /// The order's own `quantity` field — how many total the buyer is looking to acquire
    /// across (potentially many) trades, NOT the lot size of any single trade. Shown above the
    /// whisper purely as context (e.g. "they want 36 total, you have 12") — never filtered on,
    /// since fulfillability only depends on `lot_size`.
    pub(crate) listing_quantity: u32,
    /// Total owned of this exact relic/tier at the time of the check, shown alongside
    /// `listing_quantity` for the same reason.
    pub(crate) owned_quantity: u32,
}

/// Outcome of matching one relic/tier's owned quantity against its live buy-order book.
pub(crate) enum RelicMatch<'a> {
    /// Best-paying order this account can actually fulfill (lot size ≤ owned quantity, and
    /// meets `min_price` if one was given).
    Best(&'a PublicOrder),
    /// No visible buy orders exist for this tier at all.
    NoBuyOrders,
    /// Buy orders exist, but none clear the lot-size/min-price bar.
    NoFulfillableOrder,
}

/// The full result of a `RelicSellService::sync` run: every fulfillable match found,
/// already sorted best-rate-first, plus counts of the relic/tier combinations that
/// didn't produce a message (for the CLI's trailing summary line).
pub(crate) struct RelicSellResult {
    pub(crate) messages: Vec<RelicSellMessage>,
    pub(crate) no_buy_orders: u32,
    pub(crate) no_fulfillable_order: u32,
}

/// Finds the best-paying, fulfillable buy order for one relic/tier. "Fulfillable" means the
/// order's lot size (`perTrade`) doesn't exceed `owned_qty` — a buy order asking for a bigger
/// lot than we own of this exact relic/tier can't be delivered on, regardless of price.
/// Ranks by per-relic rate (`platinum / lot_size`), not raw lot total, since WFM's `platinum`
/// is the price for the whole lot, not per relic.
pub(crate) fn best_fulfillable_relic_order<'a>(
    orders: &'a [PublicOrder],
    tier: &str,
    owned_qty: u32,
    min_price: Option<u32>,
) -> RelicMatch<'a> {
    let buy_orders_for_tier = || {
        orders
            .iter()
            .filter(|o| o.is_buy() && o.visible && o.subtype.as_deref() == Some(tier))
    };
    let unit_price = |o: &PublicOrder| f64::from(o.platinum) / f64::from(o.lot_size());

    let mut candidates: Vec<&PublicOrder> = buy_orders_for_tier()
        .filter(|o| o.lot_size() <= owned_qty)
        .collect();
    if let Some(min) = min_price {
        candidates.retain(|o| unit_price(o) >= f64::from(min));
    }

    match candidates
        .iter()
        .max_by(|a, b| unit_price(a).total_cmp(&unit_price(b)))
    {
        Some(best) => RelicMatch::Best(best),
        None => {
            if buy_orders_for_tier().next().is_some() {
                RelicMatch::NoFulfillableOrder
            } else {
                RelicMatch::NoBuyOrders
            }
        }
    }
}

/// Stateless application service (aside from the network calls it makes on the caller's
/// behalf): refreshes caches, ingests/maps inventory, and matches owned relics against
/// live buy orders.
pub(crate) struct RelicSellService;

impl RelicSellService {
    /// # Errors
    /// Returns an error if caches can't be refreshed, the inventory file can't be
    /// found/decrypted, or inventory mapping fails outright. Per-item order-fetch
    /// failures are logged and skipped rather than aborting the whole run.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn sync(min_price: Option<u32>) -> AppResult<RelicSellResult> {
        mapping::update_caches().await?;

        let inventory_path = crate::app::resolve_inventory_path(None)?;
        let inventory = ingestion::ingest_inventory(&inventory_path)?;

        let client = http::shared_client();
        let mapped_items = mapping::map_inventory(&inventory, client).await?;

        // Owned quantity per (base slug, refinement) — NOT just per slug, since one slug
        // covers all four tiers (a relic's WFM slug isn't refinement-specific; refinement is
        // carried per-order as `subtype` instead). Aggregated rather than trusting one
        // MappedItem per combination, since nothing in map_inventory guarantees inventory.json
        // can't list the same game_ref across more than one array entry.
        let mut relic_qty: HashMap<(String, String), u32> = HashMap::new();
        let mut relic_names: HashMap<String, String> = HashMap::new();

        for item in mapped_items
            .iter()
            .filter(|i| i.category() == "relic" && i.quantity > 0)
        {
            let Some(tier) = &item.owned_subtype else {
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
            return Ok(RelicSellResult {
                messages: Vec::new(),
                no_buy_orders: 0,
                no_fulfillable_order: 0,
            });
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

            let orders = match wfm_client::fetch_item_orders(client, slug).await {
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

                let best = match best_fulfillable_relic_order(&orders, &tier, owned_qty, min_price)
                {
                    RelicMatch::Best(order) => order,
                    RelicMatch::NoBuyOrders => {
                        no_buy_orders += 1;
                        continue;
                    }
                    RelicMatch::NoFulfillableOrder => {
                        no_fulfillable_order += 1;
                        continue;
                    }
                };

                let Some(user) = &best.user else {
                    tseprintln!(
                        "[WARNING] Best buy order for {display_name} - {tier} at {}p has no attached trader info — skipping.",
                        best.platinum
                    );
                    continue;
                };

                messages.push(RelicSellMessage {
                    ign: user.ingame_name.clone(),
                    display_name: display_name.clone(),
                    tier: tier.clone(),
                    price: best.platinum,
                    lot_size: best.lot_size(),
                    listing_quantity: best.quantity,
                    owned_quantity: owned_qty,
                });
            }
        }

        // Sort by per-relic rate (platinum / lot_size), not raw lot total — a 6-relic lot at
        // 40p total (~6.67p/relic) should rank below a single-relic order at 10p, not above it.
        messages.sort_by(|a, b| {
            let rate = |m: &RelicSellMessage| f64::from(m.price) / f64::from(m.lot_size);
            rate(b).total_cmp(&rate(a))
        });

        Ok(RelicSellResult {
            messages,
            no_buy_orders,
            no_fulfillable_order,
        })
    }
}

#[cfg(test)]
mod relic_match_tests {
    use super::*;
    use crate::wfm_client::PublicOrderUser;

    fn buy_order(
        platinum: u32,
        per_trade: Option<u32>,
        subtype: &str,
        visible: bool,
    ) -> PublicOrder {
        PublicOrder {
            order_type: "buy".to_string(),
            platinum,
            quantity: 100,
            visible,
            rank: None,
            subtype: Some(subtype.to_string()),
            per_trade,
            user: Some(PublicOrderUser {
                ingame_name: "buyer".to_string(),
                status: None,
            }),
        }
    }

    #[test]
    fn no_orders_at_all_is_no_buy_orders() {
        let orders: Vec<PublicOrder> = vec![];
        assert!(matches!(
            best_fulfillable_relic_order(&orders, "radiant", 5, None),
            RelicMatch::NoBuyOrders
        ));
    }

    #[test]
    fn orders_exist_but_lot_too_big_is_no_fulfillable_order() {
        let orders = vec![buy_order(40, Some(6), "radiant", true)];
        assert!(matches!(
            best_fulfillable_relic_order(&orders, "radiant", 3, None),
            RelicMatch::NoFulfillableOrder
        ));
    }

    #[test]
    fn invisible_order_is_ignored() {
        let orders = vec![buy_order(100, Some(1), "radiant", false)];
        assert!(matches!(
            best_fulfillable_relic_order(&orders, "radiant", 5, None),
            RelicMatch::NoBuyOrders
        ));
    }

    #[test]
    fn picks_best_per_relic_rate_not_raw_total() {
        // 6-relic lot at 40p total (~6.67p/relic) should lose to a single-relic order at 10p.
        let orders = vec![
            buy_order(40, Some(6), "radiant", true),
            buy_order(10, Some(1), "radiant", true),
        ];
        let RelicMatch::Best(best) = best_fulfillable_relic_order(&orders, "radiant", 10, None)
        else {
            panic!("expected a fulfillable match");
        };
        assert_eq!(best.platinum, 10);
    }

    #[test]
    fn min_price_filters_out_low_rate_orders() {
        let orders = vec![buy_order(40, Some(6), "radiant", true)]; // ~6.67p/relic
        assert!(matches!(
            best_fulfillable_relic_order(&orders, "radiant", 10, Some(10)),
            RelicMatch::NoFulfillableOrder
        ));
    }

    #[test]
    fn wrong_tier_is_not_matched() {
        let orders = vec![buy_order(100, Some(1), "intact", true)];
        assert!(matches!(
            best_fulfillable_relic_order(&orders, "radiant", 5, None),
            RelicMatch::NoBuyOrders
        ));
    }
}
