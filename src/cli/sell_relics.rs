use crate::AppResult;
use crate::services::RelicSellService;

use super::{capitalize_tier, print_header, tseprintln, tsprintln};

/// `sell-relics`: matches owned relics (by refinement/tier) against the live public buy-order
/// book and prints a whisper message for the best-paying, fulfillable buy order on each one,
/// sorted descending by per-relic rate (platinum ÷ perTrade lot size).
///
/// Important WFM-specific detail, confirmed against the real API: unlike most bulk-tradable
/// items, a relic's WFM slug is NOT refinement-specific — `lith_o2_relic` alone covers Intact
/// through Radiant. Refinement is carried as each individual order's `subtype` field instead,
/// so the service fetches each distinct relic's order book exactly once and splits by
/// `subtype` locally, rather than issuing one fetch per tier.
///
/// Unlike `run_check_sets_cli`/the default pipeline, this never logs into WFM — whispering a
/// buyer only needs their in-game name off the *public* per-item order book, not an
/// authenticated `WfmClient`.
///
/// All the actual work (cache refresh, inventory import, order-book fetch, and
/// owned-vs-buy-order matching) lives in `services::RelicSellService` — this function is
/// purely presentation: run the match, then print it.
///
/// # Errors
/// Returns an error if caches can't be refreshed, the inventory file can't be found/decrypted,
/// or inventory mapping fails outright. Per-item order-fetch failures are logged and skipped
/// rather than aborting the whole run.
pub async fn run_sell_relics_cli(min_price: Option<u32>) -> AppResult<()> {
    print_header("Sell Relics — Matching Buy Orders");

    tsprintln!(
        "Refreshing caches, mapping inventory, and checking live buy orders for every owned relic (respects WFM's 3 req/s limit, so this may take a bit)..."
    );
    let result = RelicSellService::sync(min_price).await?;

    for warning in &result.warnings {
        tseprintln!("{warning}");
    }

    print_header(&format!("Whisper Messages ({})", result.messages.len()));
    if result.messages.is_empty() {
        tsprintln!(
            "(No owned relic had a fulfillable buy order{}.)",
            if min_price.is_some() {
                " at or above the requested minimum price"
            } else {
                ""
            }
        );
    }
    for m in &result.messages {
        let tier = capitalize_tier(&m.tier);
        tsprintln!(
            "{} - {}  (listing wants {}, you own {})",
            m.display_name,
            tier,
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
            tier,
            m.price
        );
    }

    if result.no_buy_orders > 0 || result.no_fulfillable_order > 0 {
        tsprintln!(
            "\n({} relic/tier combo(s) had no visible buy orders at all; {} had buy orders but none we could fulfill at the requested lot size/min price.)",
            result.no_buy_orders,
            result.no_fulfillable_order
        );
    }

    Ok(())
}
