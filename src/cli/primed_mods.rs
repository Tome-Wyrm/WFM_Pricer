use std::error::Error;

use super::{
    PRIMED_MODS, PrimedPrice, calculate_weighted_average, fetch_statistics, print_header,
    recent_volume, tseprintln, tsprintln,
};

/// # Errors
/// Returns an error if caches can't be loaded, the WFM items cache is missing,
/// or network requests fail.
pub async fn run_primed_mod_prices(min_rank: bool) -> Result<(), Box<dyn Error>> {
    print_header(if min_rank {
        "Primed Mod Prices (Unranked)"
    } else {
        "Primed Mod Prices (Maxed)"
    });

    tsprintln!("Fetching current market statistics...\n");

    // Max rank varies per mod (most Primed set mods cap at 5, but some — e.g. the
    // ammo-mutation/ammo-chain/ammo-stock mods — cap lower or higher). We used to
    // hardcode Some(10) here, which silently returned (0.0, 0) for every mod whose
    // real max rank wasn't exactly 10, since calculate_weighted_average/recent_volume
    // filter WFM stats on an exact rank match. Pull the real maxRank from the WFM
    // items cache (keyed by slug) instead, so this can't drift out of sync again.
    // Still needed even in --min-rank mode, just to confirm each slug is a known item.
    let (_wfcd_by_ref, _wfm_by_ref, _wfm_by_name, wfm_by_slug) =
        crate::mapping::load_lookup_tables()?;

    let mut prices = Vec::<PrimedPrice>::new();

    for primed in PRIMED_MODS {
        let Some(raw_max_rank) = wfm_by_slug.get(primed.slug).and_then(|item| item.max_rank) else {
            tseprintln!(
                "[WARNING] Could not resolve max rank for {} ('{}') from WFM items cache — skipping.",
                primed.name,
                primed.slug
            );
            continue;
        };
        // WFM's statistics rank field is u32 while max_rank on WfmItem is also u32,
        // but calculate_weighted_average/recent_volume take Option<u8> — narrow safely.
        let Ok(raw_max_rank) = u8::try_from(raw_max_rank) else {
            tseprintln!(
                "[WARNING] Max rank {} for {} doesn't fit in u8 — skipping.",
                raw_max_rank,
                primed.name
            );
            continue;
        };
        // Unranked is always rank 0 regardless of the mod's max rank; --min-rank just
        // pins the target to that instead of raw_max_rank.
        let target_rank: u8 = if min_rank { 0 } else { raw_max_rank };

        // calculate_weighted_average/recent_volume now self-correct if this guessed rank
        // doesn't match any real statistics row but null-ranked rows exist instead (see
        // resolve_target_rank in pricing.rs) — e.g. items like "Peculiar Audience" that WFM
        // tracks with rank: null on every row rather than numeric ranks.
        match fetch_statistics(primed.slug).await {
            Ok(stats) => {
                let (price, _) = calculate_weighted_average(&stats, Some(target_rank));
                let (volume, _) = recent_volume(&stats, Some(target_rank), 30);

                prices.push(PrimedPrice {
                    name: primed.name,
                    price,
                    volume,
                });
            }

            Err(err) => {
                tseprintln!("[WARNING] Failed to fetch {}: {}", primed.name, err);
            }
        }
    }

    prices.sort_by(|a, b| {
        b.price
            .partial_cmp(&a.price)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    tsprintln!("{:<4} {:<34} {:>10} {:>10}", "#", "Mod", "Price", "30d Vol");

    tsprintln!("{}", "-".repeat(64));

    for (i, mod_price) in prices.iter().enumerate() {
        tsprintln!(
            "{:<4} {:<34} {:>10.1} {:>10}",
            i + 1,
            mod_price.name,
            mod_price.price,
            mod_price.volume,
        );
    }

    Ok(())
}

// ── Blacklist / Keeplist helpers ────────────────────────────────────────────
