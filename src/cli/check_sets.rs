use crate::AppResult;
use crate::services::{SetAnalysisService, SetPricingResult};

use super::{print_header, tseprintln, tsprintln};

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
/// results are printed, so the completeness detection itself — `mapping::find_incomplete_sets`,
/// which is driven entirely by "does this build have a sellable WFM Set listing", not by "can
/// this be crafted from a blueprint" — can be sanity-checked independently of pricing. Sets that
/// can't be fully priced (no current orders on one side or the other) are still shown in the
/// final table, just with `N/A` in place of a number and a reason, rather than disappearing.
///
/// Does not place any orders — this is a read-only profitability report. Sunk cost of the
/// parts you already own is intentionally not counted against the profit figure.
///
/// All the actual work (cache refresh, inventory import, completeness detection, and
/// per-Set pricing) lives in `services::SetAnalysisService` — this function is purely
/// presentation: run the analysis, then print it.
///
/// # Errors
/// Returns an error if caches can't be refreshed/loaded, the inventory can't be ingested, or
/// inventory-to-WFM mapping fails.
pub async fn run_check_sets_cli(min_profit: Option<f64>) -> AppResult<()> {
    print_header("Incomplete Set Profitability Check");

    tsprintln!(
        "Refreshing caches, mapping inventory, and checking current buy/sell orders for every incomplete Set (respects WFM's 3 req/s limit, so this may take a bit)..."
    );
    let analysis = SetAnalysisService::analyze().await?;

    if analysis.incomplete.is_empty() {
        tsprintln!(
            "No incomplete Sets found — every Set you own parts of is either complete already or you don't own any of its parts yet."
        );
        return Ok(());
    }

    // Report everything the completeness check found, unconditionally, before printing any
    // pricing results. This is the audit trail for `find_incomplete_sets` itself — if
    // something looks wrong here (a Set that shouldn't be sellable, a missing-part quantity
    // that looks off, etc.), that's a detection-logic bug, independent of anything the
    // pricing pass below does.
    print_header(&format!(
        "Incomplete Sets Found ({})",
        analysis.incomplete.len()
    ));
    for set in &analysis.incomplete {
        let parts: Vec<String> = set
            .missing
            .iter()
            .map(|(name, deficit)| format!("{deficit}x {name}"))
            .collect();
        tsprintln!("  {}: needs {}", set.name, parts.join(", "));
    }
    tsprintln!();

    let mut priced = Vec::new();
    for result in analysis.results {
        match result {
            SetPricingResult::Priced(set) => priced.push(set),
            SetPricingResult::Unpriced {
                name,
                reason,
                is_bug,
            } => {
                if is_bug {
                    tseprintln!("[WARNING] '{name}' {reason}.");
                } else {
                    tsprintln!("{name}: could not price — {reason}.");
                }
            }
        }
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
