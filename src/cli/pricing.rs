use super::{print_header, tsprintln};

/// Prints the top mod-upgrade suggestions (already sorted/truncated by
/// `pricing::build_priced_candidates`, but re-sorted/truncated here defensively in case a
/// future caller passes an unsorted list).
///
/// The pricing/ranking algorithm itself — `LiveStatsSource`, `build_priced_candidates`,
/// `sort_candidates` — moved to top-level `pricing.rs` (Architecture Evolution Plan
/// Phase 1.5); this function stays here because it's actual presentation, a formatted
/// table print.
pub(crate) fn print_upgrade_suggestions(suggestions: &[(String, f64, u32, u32, f64)]) {
    let mut sorted = suggestions.to_vec();
    sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(15);

    print_header("Mod Upgrade Suggestions (Best Endo Value × Volume)");
    tsprintln!(
        "\x1B[1m  {:<35} | {:<14} | {:<12} | {:<10} | Score\x1B[0m",
        "Mod",
        "Δ Plat (→max)",
        "Endo Cost",
        "30d Vol"
    );
    tsprintln!("  {}", "-".repeat(82));
    for (name, delta, endo, vol, score) in &sorted {
        tsprintln!("  {name:<35} | {delta:<14.1} | {endo:<12} | {vol:<10} | {score:.4}");
    }
    tsprintln!();
}
