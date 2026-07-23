use crate::AppResult;
use crate::services::PrimedModPriceService;

use super::{print_header, tseprintln, tsprintln};

/// # Errors
/// Returns an error if the WFM items cache can't be loaded (see
/// `services::PrimedModPriceService`). Per-mod lookup/fetch failures are logged as warnings
/// and skipped rather than aborting the whole run.
pub async fn run_primed_mod_prices(min_rank: bool) -> AppResult<()> {
    print_header(if min_rank {
        "Primed Mod Prices (Unranked)"
    } else {
        "Primed Mod Prices (Maxed)"
    });

    tsprintln!("Fetching current market statistics...\n");

    let result = PrimedModPriceService::fetch(min_rank).await?;

    for warning in &result.warnings {
        tseprintln!("[WARNING] {}", warning.message);
    }

    tsprintln!("{:<4} {:<34} {:>10} {:>10}", "#", "Mod", "Price", "30d Vol");
    tsprintln!("{}", "-".repeat(64));

    for (i, mod_price) in result.prices.iter().enumerate() {
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
