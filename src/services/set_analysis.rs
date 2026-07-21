//! `SetAnalysisService` — finds incomplete Sets and prices out whether finishing them is
//! profitable, without printing anything itself.
//!
//! Extracted from `cli::check_sets::run_check_sets_cli`, which used to interleave this
//! logic with `tsprintln!`/`tseprintln!` calls throughout — making the pricing algorithm
//! hard to read on its own and impossible to reuse from anything other than that one CLI
//! command. The CLI is now just: call [`SetAnalysisService::analyze`], then format the
//! result.
//!
//! Network calls are rate-limited the same way the original code was (a 350ms sleep after
//! every `wfm_client::fetch_item_orders` call, respecting WFM's 3 req/s limit) — that
//! detail lives here now, not in the CLI.

use std::time::Duration;
use tokio::time::sleep;

use crate::mapping::IncompleteSet;
use crate::{AppResult, http, mapping, services::InventoryImportService, wfm_client};

/// A Set the user owns at least one component of but hasn't finished assembling, with
/// names already resolved for display (callers don't need `wfcd_by_ref` themselves).
pub struct IncompleteSetInfo {
    pub name: String,
    /// (part name, deficit quantity)
    pub missing: Vec<(String, u32)>,
}

/// One priced-out missing part: name, quantity needed, unit cost, and whether that unit
/// cost was the current best *buy* order (competitive bid) or a fallback to the current
/// best *sell* order (paying the ask, because no buy orders existed to bid against).
pub type PricedComponent = (String, u32, u32, bool);

pub struct PricedIncompleteSet {
    pub name: String,
    pub missing: Vec<PricedComponent>,
    pub total_cost: u32,
    pub set_sell_price: u32,
    pub profit: f64,
}

/// The outcome of trying to price one incomplete Set.
pub enum SetPricingResult {
    Priced(PricedIncompleteSet),
    /// Couldn't be priced (no orders on one side, a part missing from the WFM cache, a
    /// failed network request, ...). `is_bug` marks the one case that should never happen
    /// in practice — a Set that passed completeness detection but has no resolvable WFM
    /// Set listing — so the CLI can flag it more loudly than an ordinary pricing gap.
    Unpriced {
        name: String,
        reason: String,
        is_bug: bool,
    },
}

/// Full result of a `--check-sets` analysis pass: every incomplete Set found (before any
/// pricing, so completeness detection can be sanity-checked on its own), plus a pricing
/// result for each one, in the same order they were found.
pub struct SetAnalysis {
    pub incomplete: Vec<IncompleteSetInfo>,
    pub results: Vec<SetPricingResult>,
}

/// Stateless application service: refreshes caches, imports the current inventory, finds
/// incomplete Sets, and prices out whether completing each one is profitable.
///
/// Prints nothing — see [`SetAnalysis`] for what it returns instead.
pub struct SetAnalysisService;

impl SetAnalysisService {
    /// # Errors
    /// Returns an error if caches can't be refreshed/loaded, the inventory can't be
    /// ingested, or inventory-to-WFM mapping fails. Per-Set/per-part pricing failures
    /// (network errors, missing orders, ...) are *not* errors — they show up as
    /// `SetPricingResult::Unpriced` entries instead, same as before this was a service.
    #[allow(clippy::too_many_lines)]
    pub async fn analyze() -> AppResult<SetAnalysis> {
        mapping::update_caches().await?;
        let imported = InventoryImportService::import(None).await?;
        let client = http::shared_client();

        let incomplete: Vec<IncompleteSet> = mapping::find_incomplete_sets(
            &imported.mapped,
            &imported.requirements,
            &imported.wfcd_by_ref,
            &imported.wfm_by_ref,
            &imported.wfm_by_name,
        );

        let incomplete_info = incomplete
            .iter()
            .map(|set| {
                let name = imported
                    .wfcd_by_ref
                    .get(&set.build_unique)
                    .map_or(set.build_unique.as_str(), |w| w.name.as_str())
                    .to_string();
                let missing = set
                    .missing
                    .iter()
                    .map(|c| (c.name.clone(), c.deficit))
                    .collect();
                IncompleteSetInfo { name, missing }
            })
            .collect();

        let mut results = Vec::with_capacity(incomplete.len());

        for set in &incomplete {
            let Some(wfcd_item) = imported.wfcd_by_ref.get(&set.build_unique) else {
                continue;
            };
            // find_incomplete_sets already only returns builds with a resolvable WFM Set
            // listing, so this should always succeed — surfaced as a `is_bug` Unpriced
            // result (not silently dropped: the Set still shows up in `incomplete` above)
            // if the two ever disagree.
            let Some(set_wfm_item) =
                mapping::resolve_set_item(&wfcd_item.name, &imported.wfm_by_name)
            else {
                results.push(SetPricingResult::Unpriced {
                    name: wfcd_item.name.clone(),
                    reason: "passed completeness detection but has no resolvable WFM Set listing — this is a bug, please report it".to_string(),
                    is_bug: true,
                });
                continue;
            };

            let mut unpriced_reason: Option<String> = None;

            let set_sell_price =
                match wfm_client::fetch_item_orders(client, &set_wfm_item.slug).await {
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

                let Some(wfm_comp) = imported.wfm_by_ref.get(&comp.unique_name) else {
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
                    // Worse-case cost estimate (paying the ask, not getting filled at your
                    // own bid), so it's flagged with `used_ask` for the caller to mark.
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
                results.push(SetPricingResult::Unpriced {
                    name: wfcd_item.name.clone(),
                    reason,
                    is_bug: false,
                });
                continue;
            }

            // Reachable only once set_sell_price is confirmed Some above.
            let set_sell_price = set_sell_price.unwrap_or(0);
            let profit = f64::from(set_sell_price) - f64::from(total_cost);
            results.push(SetPricingResult::Priced(PricedIncompleteSet {
                name: wfcd_item.name.clone(),
                missing: priced_missing,
                total_cost,
                set_sell_price,
                profit,
            }));
        }

        Ok(SetAnalysis {
            incomplete: incomplete_info,
            results,
        })
    }
}
