// src/vendor/scoring.rs
//! Cost classification (Phase E) and the score/rank/filter pipeline (Phase F).
use super::matching::{MappedOffering, MappedVendor};
use super::metadata::CostMode;
use super::raw::PriceSpec;
use crate::AppResult;
use crate::config;
use serde::{Deserialize, Serialize};

// ================== Phase E: Cost model & multi-currency classification ==================

// ---- E1: Cost type ----

/// The classified cost of a single offering, derived from its `PriceSpec` (Phase A3)
/// plus the owning vendor's `cost_mode` (Phase C). `Unclassified` is the tripwire case:
/// a multi-currency `PriceSpec::Multi` whose vendor has no `cost_mode` override. Those
/// are deliberately *not* guessed at — they're excluded from scoring and surfaced in
/// the "needs classification" list alongside the match-coverage report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Cost {
    Single(String, f64),
    AnyOf(Vec<(String, f64)>),
    AllOf(Vec<(String, f64)>),
    Unclassified(Vec<(String, f64)>),
}

/// Builds a `Cost` from an offering's `PriceSpec` and its vendor's `cost_mode`.
/// `PriceSpec::Single` always classifies as `Cost::Single` regardless of `cost_mode`.
/// `PriceSpec::Multi` maps to `AnyOf`/`AllOf` per the vendor's declared mode, or
/// `Unclassified` if the vendor never declared one — this function refuses to guess.
#[must_use]
pub fn classify_cost(price: &PriceSpec, cost_mode: &CostMode) -> Cost {
    match price {
        PriceSpec::Single(cur, amt) => Cost::Single(cur.clone(), *amt),
        PriceSpec::Multi(pairs) => match cost_mode {
            CostMode::AnyOf => Cost::AnyOf(pairs.clone()),
            CostMode::AllOf => Cost::AllOf(pairs.clone()),
            CostMode::Single => Cost::Unclassified(pairs.clone()),
        },
    }
}

/// Convenience wrapper: classifies every offering on a `MappedVendor`, pairing each
/// with its `Cost` for downstream scoring/reporting.
#[must_use]
pub fn classify_vendor_offerings(vendor: &MappedVendor) -> Vec<(&MappedOffering, Cost)> {
    vendor
        .offerings
        .iter()
        .map(|o| (o, classify_cost(&o.price, &vendor.cost_mode)))
        .collect()
}

/// Names of offerings across all vendors whose `Cost` came out `Unclassified` — the
/// "needs classification" list to surface alongside the match-coverage report (D4).
#[must_use]
pub fn unclassified_offerings(vendors: &[MappedVendor]) -> Vec<(String, String)> {
    vendors
        .iter()
        .flat_map(|v| {
            classify_vendor_offerings(v)
                .into_iter()
                .filter(|(_, cost)| matches!(cost, Cost::Unclassified(_)))
                .map(|(o, _)| (v.key.clone(), o.name.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

// ---- E2: Scoring against Cost ----

/// One row of the eventual ranking table: a single (currency, score) pairing for an
/// offering, or a note in place of a score when a clean per-currency score isn't
/// meaningful (the `AllOf` case).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRow {
    pub currency: String,
    pub amount: f64,
    /// `weighted_avg_price / amount`. `None` when `note` is set instead (`AllOf`).
    pub score: Option<f64>,
    /// Set for `AllOf` rows: "also requires: X amount of Y" for every *other*
    /// currency required alongside this row's own.
    pub note: Option<String>,
}

/// Expands a `Cost` into its `CostRow`s given a plat price for the item being bought.
/// - `Single`/`AnyOf`: one row per currency, each with a clean `score`.
/// - `AllOf`: one row per currency too (so the offering still shows up under every
///   currency's table), but with a `note` instead of a score — since the currencies
///   aren't fungible with each other, there's no honest single number to rank by
///   across vendors; manufacturing one would only make sense if the buyer already
///   has surplus of every required currency, which isn't a safe assumption.
/// - `Unclassified`: no rows — excluded from scoring entirely (E1).
#[must_use]
pub fn cost_rows(cost: &Cost, weighted_avg_price: f64) -> Vec<CostRow> {
    match cost {
        Cost::Single(cur, amt) => vec![CostRow {
            currency: cur.clone(),
            amount: *amt,
            score: Some(weighted_avg_price / amt),
            note: None,
        }],
        Cost::AnyOf(pairs) => pairs
            .iter()
            .map(|(cur, amt)| CostRow {
                currency: cur.clone(),
                amount: *amt,
                score: Some(weighted_avg_price / amt),
                note: None,
            })
            .collect(),
        Cost::AllOf(pairs) => pairs
            .iter()
            .map(|(cur, amt)| {
                let others: Vec<String> = pairs
                    .iter()
                    .filter(|(c, _)| c != cur)
                    .map(|(c, a)| format!("{a} {c}"))
                    .collect();
                let note = if others.is_empty() {
                    None
                } else {
                    Some(format!("also requires: {}", others.join(", ")))
                };
                CostRow {
                    currency: cur.clone(),
                    amount: *amt,
                    score: None,
                    note,
                }
            })
            .collect(),
        Cost::Unclassified(_) => Vec::new(),
    }
}

// ================== Phase F: Pricing/scoring engine ==================

// ---- F2: Saturation as a filter, not a score multiplier ----

/// F2: `true` if `saturation` should be filtered out of the ranking table under the
/// given `--max-saturation` threshold. `None` means no filtering — the caller still
/// always displays the saturation column, this only gates inclusion in the ranked
/// results. Kept as its own tiny function so the filter and the display concern stay
/// decoupled (per the plan: "table output always shows a saturation column;
/// filtering only kicks in when the flag is passed").
#[must_use]
pub fn exceeds_saturation_cap(saturation: f64, max_saturation: Option<f64>) -> bool {
    max_saturation.is_some_and(|cap| saturation > cap)
}

// ---- F3: Relic refinement check ----
//
// Investigated 2026-xx-xx against the real stats endpoint for `axi_a1_relic`: the
// `statistics_closed`/`statistics_live` payloads returned by `/v1/items/{slug}/statistics`
// carry `mod_rank` (always `null` for relics) but no subtype/refinement field at all —
// Intact vs Exceptional/Flawless/Radiant orders aren't broken out anywhere in the
// per-day aggregates, only in live per-order listings (which we don't fetch here).
// Decision: exclude the `Relic` category from vendor-rank scoring for v1 rather than
// silently averaging across refinement levels (which would understate Radiant value
// and overstate Intact value). Revisit if WFM ever adds a refinement dimension to the
// statistics endpoint. See `target_rank_for`/`is_tradeable_category` — add a `Relic`
// exclusion there (or in the F4 filter below) if/when this needs enforcing in code;
// as of this writing the real wiki dump hasn't produced a `Relic`-category offering
// in practice, so there's nothing to exclude yet and no test to write.

// ---- F4: score computation + ranking ----

/// One input row to the F4 scorer: everything needed to compute a score and apply
/// the demand floor, already resolved from a `MappedOffering` + its WFM price stats.
/// Kept separate from the network-fetching path so the actual scoring/sorting/
/// filtering logic is directly unit-testable with literal data (per the plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreInput {
    pub label: String,
    pub currency: String,
    pub weighted_avg_price: f64,
    pub cost_amount: f64,
    pub daily_volume: f64,
    pub saturation: f64,
}

/// One output row of the F4 ranking: a scored, demand-floor-passed, saturation-
/// filtered candidate, sorted descending by `score` by the caller of `rank_by_score`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoredRow {
    pub label: String,
    pub currency: String,
    pub score: f64,
    pub daily_volume: f64,
    pub saturation: f64,
}

/// F4: `score = weighted_avg_price / cost_amount` for each input, dropping rows below
/// `config::MIN_DAILY_VOLUME` (universal demand floor, per F1) and rows over
/// `max_saturation` when one is given (F2), then sorting the rest descending by score.
/// Pure/sync — no cache or network access — so it's testable with literal fixture data.
#[must_use]
pub fn rank_by_score(inputs: Vec<ScoreInput>, max_saturation: Option<f64>) -> Vec<ScoredRow> {
    let mut rows: Vec<ScoredRow> = inputs
        .into_iter()
        .filter(|i| i.daily_volume >= config::MIN_DAILY_VOLUME)
        .filter(|i| !exceeds_saturation_cap(i.saturation, max_saturation))
        .map(|i| ScoredRow {
            label: i.label,
            currency: i.currency,
            score: i.weighted_avg_price / i.cost_amount,
            daily_volume: i.daily_volume,
            saturation: i.saturation,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

/// Full vendor-rank pipeline row: a `CostRow` (E2) that's been priced against real WFM
/// stats and passed through the F4 scorer/filters (or, for `AllOf` rows, passed through
/// unscored with its note intact — those never had a `score` to filter on in the first
/// place, so the demand floor is still applied via `daily_volume` but saturation/score
/// sorting doesn't apply to them).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedOffering {
    pub vendor_key: String,
    pub vendor_name: String,
    pub offering_name: String,
    pub category: String,
    pub currency: String,
    pub amount: f64,
    pub score: Option<f64>,
    pub note: Option<String>,
    pub weighted_avg_price: f64,
    pub daily_volume: f64,
    pub saturation: f64,
}

/// Builds the full ranked-offering table across every non-excluded vendor: fetches
/// live WFM stats per matched offering, computes weighted-average price / recent
/// volume / saturation (reusing `pricing::calculate_weighted_average`,
/// `pricing::recent_volume`, `pricing::calculate_saturation_ratio`), classifies each
/// offering's cost (E1/E2), applies the demand floor + optional `--max-saturation`
/// filter, and returns rows sorted descending by score. `AllOf` rows are included
/// without a score (per E2) but are still demand-floor- and saturation-filtered.
///
/// # Errors
/// Returns an error only if a WFM stats fetch itself errors out; individual offerings
/// with no market data or no WFM match are simply skipped, not treated as failures.
pub async fn rank_offerings(
    vendors: &[MappedVendor],
    max_saturation: Option<f64>,
) -> AppResult<Vec<RankedOffering>> {
    use std::convert::TryFrom;

    let mut scored_inputs: Vec<(ScoreInput, RankedOffering)> = Vec::new();
    let mut unscored: Vec<RankedOffering> = Vec::new();

    for vendor in vendors {
        if vendor.excluded {
            continue;
        }
        for offering in &vendor.offerings {
            let Some(slug) = offering.wfm_slug.as_deref() else {
                continue; // unmatched — nothing to price
            };

            let stats = crate::pricing::fetch_statistics(slug).await?;
            let target_rank_u8 = offering.target_rank.and_then(|r| u8::try_from(r).ok());

            let (weighted_avg_price, _) =
                crate::pricing::calculate_weighted_average(&stats, target_rank_u8);
            if weighted_avg_price <= 0.0 {
                continue; // no market price
            }

            let (vol_30d, _) = crate::pricing::recent_volume(&stats, target_rank_u8, 30);
            let daily_volume = f64::from(vol_30d) / 30.0;
            let saturation = crate::pricing::calculate_saturation_ratio(&stats, target_rank_u8);

            let cost = classify_cost(&offering.price, &vendor.cost_mode);
            for row in cost_rows(&cost, weighted_avg_price) {
                let ranked = RankedOffering {
                    vendor_key: vendor.key.clone(),
                    vendor_name: vendor.name.clone(),
                    offering_name: offering.name.clone(),
                    category: offering.category.clone(),
                    currency: row.currency.clone(),
                    amount: row.amount,
                    score: row.score,
                    note: row.note.clone(),
                    weighted_avg_price,
                    daily_volume,
                    saturation,
                };
                match row.score {
                    Some(_) => scored_inputs.push((
                        ScoreInput {
                            label: format!("{} — {}", vendor.name, offering.name),
                            currency: row.currency,
                            weighted_avg_price,
                            cost_amount: row.amount,
                            daily_volume,
                            saturation,
                        },
                        ranked,
                    )),
                    None => {
                        // AllOf: no clean score to sort by, but still demand-floor- and
                        // saturation-filtered like everything else.
                        if daily_volume >= config::MIN_DAILY_VOLUME
                            && !exceeds_saturation_cap(saturation, max_saturation)
                        {
                            unscored.push(ranked);
                        }
                    }
                }
            }
        }
    }

    let inputs: Vec<ScoreInput> = scored_inputs.iter().map(|(i, _)| i.clone()).collect();
    let scored_rows = rank_by_score(inputs, max_saturation);

    // Re-attach each surviving ScoreInput to its RankedOffering by (label, currency) —
    // cheap and correct since both are built from the same offering in lockstep above.
    let mut result: Vec<RankedOffering> = Vec::new();
    for scored in &scored_rows {
        if let Some((_, ranked)) = scored_inputs
            .iter()
            .find(|(i, _)| i.label == scored.label && i.currency == scored.currency)
        {
            result.push(ranked.clone());
        }
    }
    result.extend(unscored);
    Ok(result)
}

#[cfg(test)]
mod cost_model_tests {
    use super::*;

    // ---- E1: classify_cost ----

    #[test]
    fn single_currency_price_classifies_as_single_regardless_of_cost_mode() {
        let price = PriceSpec::Single("Credits".to_string(), 5000.0);
        assert_eq!(
            classify_cost(&price, &CostMode::Single),
            Cost::Single("Credits".to_string(), 5000.0)
        );
        assert_eq!(
            classify_cost(&price, &CostMode::AllOf),
            Cost::Single("Credits".to_string(), 5000.0)
        );
    }

    #[test]
    fn unearth_citrine_style_multi_price_classifies_as_all_of() {
        let price = PriceSpec::Multi(vec![
            ("Entrati Standing".to_string(), 5000.0),
            ("Credits".to_string(), 10000.0),
        ]);
        let cost = classify_cost(&price, &CostMode::AllOf);
        assert_eq!(
            cost,
            Cost::AllOf(vec![
                ("Entrati Standing".to_string(), 5000.0),
                ("Credits".to_string(), 10000.0),
            ])
        );
    }

    #[test]
    fn operational_supply_style_multi_price_classifies_as_any_of() {
        let price = PriceSpec::Multi(vec![
            ("Cetus Standing".to_string(), 2500.0),
            ("Credits".to_string(), 5000.0),
        ]);
        let cost = classify_cost(&price, &CostMode::AnyOf);
        assert_eq!(
            cost,
            Cost::AnyOf(vec![
                ("Cetus Standing".to_string(), 2500.0),
                ("Credits".to_string(), 5000.0),
            ])
        );
    }

    #[test]
    fn hunhow_style_multi_price_classifies_as_all_of() {
        let price = PriceSpec::Multi(vec![
            ("Holdfast Standing".to_string(), 15000.0),
            ("Credits".to_string(), 25000.0),
        ]);
        let cost = classify_cost(&price, &CostMode::AllOf);
        assert_eq!(
            cost,
            Cost::AllOf(vec![
                ("Holdfast Standing".to_string(), 15000.0),
                ("Credits".to_string(), 25000.0),
            ])
        );
    }

    #[test]
    fn unconfigured_multi_currency_offering_comes_out_unclassified() {
        let price = PriceSpec::Multi(vec![
            ("Some Standing".to_string(), 1000.0),
            ("Credits".to_string(), 2000.0),
        ]);
        let cost = classify_cost(&price, &CostMode::Single);
        assert_eq!(
            cost,
            Cost::Unclassified(vec![
                ("Some Standing".to_string(), 1000.0),
                ("Credits".to_string(), 2000.0),
            ])
        );
    }

    // ---- E2: cost_rows ----

    #[test]
    fn single_cost_produces_one_scored_row() {
        let cost = Cost::Single("Credits".to_string(), 5000.0);
        let rows = cost_rows(&cost, 100.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].currency, "Credits");
        assert_eq!(rows[0].amount, 5000.0);
        assert_eq!(rows[0].score, Some(100.0 / 5000.0));
        assert_eq!(rows[0].note, None);
    }

    #[test]
    fn any_of_cost_produces_one_scored_row_per_currency() {
        let cost = Cost::AnyOf(vec![
            ("Cetus Standing".to_string(), 2500.0),
            ("Credits".to_string(), 5000.0),
        ]);
        let rows = cost_rows(&cost, 100.0);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert!(row.score.is_some());
            assert_eq!(row.note, None);
        }
        assert_eq!(rows[0].score, Some(100.0 / 2500.0));
        assert_eq!(rows[1].score, Some(100.0 / 5000.0));
    }

    #[test]
    fn all_of_cost_produces_noted_unscored_rows_per_currency() {
        let cost = Cost::AllOf(vec![
            ("Entrati Standing".to_string(), 5000.0),
            ("Credits".to_string(), 10000.0),
        ]);
        let rows = cost_rows(&cost, 100.0);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(
                row.score, None,
                "AllOf rows must not manufacture a fake score"
            );
            assert!(row.note.is_some());
        }
        assert_eq!(
            rows[0].note.as_deref(),
            Some("also requires: 10000 Credits")
        );
        assert_eq!(
            rows[1].note.as_deref(),
            Some("also requires: 5000 Entrati Standing")
        );
    }

    #[test]
    fn unclassified_cost_produces_no_rows() {
        let cost = Cost::Unclassified(vec![
            ("Some Standing".to_string(), 1000.0),
            ("Credits".to_string(), 2000.0),
        ]);
        let rows = cost_rows(&cost, 100.0);
        assert!(rows.is_empty());
    }
}

#[cfg(test)]

mod phase_f_scoring_tests {
    use super::*;

    fn input(label: &str, price: f64, cost: f64, volume: f64) -> ScoreInput {
        ScoreInput {
            label: label.to_string(),
            currency: "Credits".to_string(),
            weighted_avg_price: price,
            cost_amount: cost,
            daily_volume: volume,
            saturation: 0.0,
        }
    }

    // ---- F2: saturation filter ----

    #[test]
    fn no_cap_never_excludes() {
        assert!(!exceeds_saturation_cap(50.0, None));
    }

    #[test]
    fn saturation_at_or_below_cap_is_kept() {
        assert!(!exceeds_saturation_cap(1.0, Some(1.0)));
        assert!(!exceeds_saturation_cap(0.5, Some(1.0)));
    }

    #[test]
    fn saturation_above_cap_is_excluded() {
        assert!(exceeds_saturation_cap(1.01, Some(1.0)));
    }

    // ---- F4: score + rank + demand floor ----

    #[test]
    fn scores_and_sorts_descending() {
        // A: 100/10 = 10.0, B: 100/2 = 50.0, C: 100/25 = 4.0 — all clear the demand floor.
        let inputs = vec![
            input("A", 100.0, 10.0, config::MIN_DAILY_VOLUME),
            input("B", 100.0, 2.0, config::MIN_DAILY_VOLUME),
            input("C", 100.0, 25.0, config::MIN_DAILY_VOLUME),
        ];
        let rows = rank_by_score(inputs, None);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["B", "A", "C"]);
        assert_eq!(rows[0].score, 50.0);
    }

    #[test]
    fn drops_rows_below_demand_floor() {
        let inputs = vec![
            input("Illiquid", 100.0, 10.0, config::MIN_DAILY_VOLUME - 0.1),
            input("Liquid", 100.0, 10.0, config::MIN_DAILY_VOLUME),
        ];
        let rows = rank_by_score(inputs, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Liquid");
    }

    #[test]
    fn drops_rows_over_max_saturation() {
        let mut over = input("Oversaturated", 100.0, 10.0, config::MIN_DAILY_VOLUME);
        over.saturation = 5.0;
        let mut under = input("Fine", 100.0, 10.0, config::MIN_DAILY_VOLUME);
        under.saturation = 0.5;
        let rows = rank_by_score(vec![over, under], Some(1.0));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Fine");
    }
}
