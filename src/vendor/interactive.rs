// src/vendor/interactive.rs
//! CLI entry point for vendor mode: the location picker, ranked-table printing, and
//! `run_vendor_cli` orchestration (Phase G2/G3).
use super::matching::{MappedVendor, print_match_report};
use super::scoring::RankedOffering;
use crate::AppResult;
use crate::services::VendorAnalysisService;
use crate::{tsprint, tsprintln};
use std::fs;
use std::io::Write;

// ================== Phase G2/G3: location picker + ranked-table output ==================

/// One node of the nav tree derived from `MappedVendor::location` (split on `/`).
/// Purely derived data — never hand-maintained — so it can't drift from
/// `config/vendors.toml`.
#[derive(Debug, Default)]
struct LocTree {
    children: std::collections::BTreeMap<String, LocTree>,
    /// Display entries (group name, or vendor name for standalone vendors) available
    /// at exactly this node in the tree.
    entries: std::collections::BTreeSet<String>,
}

/// The name to show/select for a vendor: its `group` if pooled, else its own name.
fn entry_name(vendor: &MappedVendor) -> &str {
    vendor.group.as_deref().unwrap_or(vendor.name.as_str())
}

/// Builds the nav tree from every non-excluded vendor's `location` field.
fn build_location_tree(vendors: &[MappedVendor]) -> LocTree {
    let mut root = LocTree::default();
    for vendor in vendors {
        if vendor.excluded {
            continue;
        }
        let Some(location) = &vendor.location else {
            continue;
        };
        let mut node = &mut root;
        for segment in location.split('/').filter(|s| !s.is_empty()) {
            node = node.children.entry(segment.to_string()).or_default();
        }
        node.entries.insert(entry_name(vendor).to_string());
    }
    root
}

/// Every distinct entry name reachable from `node` (this node and all descendants) —
/// used for "0 = print all" at any level.
fn all_entries(node: &LocTree, out: &mut std::collections::BTreeSet<String>) {
    out.extend(node.entries.iter().cloned());
    for child in node.children.values() {
        all_entries(child, out);
    }
}

/// Resolves a selected entry name (group name, or standalone vendor name) to the
/// `MappedVendor`s it corresponds to.
fn vendors_for_entry<'a>(name: &str, vendors: &'a [MappedVendor]) -> Vec<&'a MappedVendor> {
    vendors
        .iter()
        .filter(|v| !v.excluded && entry_name(v) == name)
        .collect()
}

/// Walks the interactive nested menu starting at `node`, returning the set of
/// resolved vendors the user picked (either one entry, or "0" for everything under
/// the current node).
fn interactive_picker<'a>(
    root: &LocTree,
    vendors: &'a [MappedVendor],
) -> AppResult<Vec<&'a MappedVendor>> {
    let mut node = root;
    loop {
        // Build the numbered menu: child directories first, then leaf entries at this level.
        let child_names: Vec<&String> = node.children.keys().collect();
        let leaf_names: Vec<&String> = node.entries.iter().collect();

        tsprintln!("\n0. Print all");
        let mut idx = 1;
        for name in &child_names {
            tsprintln!("{idx}. {name}/");
            idx += 1;
        }
        for name in &leaf_names {
            tsprintln!("{idx}. {name}");
            idx += 1;
        }

        tsprint!("\nSelect an option: ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let choice: usize = input.trim().parse().map_err(|_| "Invalid number")?;

        if choice == 0 {
            let mut names = std::collections::BTreeSet::new();
            all_entries(node, &mut names);
            let mut result = Vec::new();
            for name in names {
                result.extend(vendors_for_entry(&name, vendors));
            }
            return Ok(result);
        }

        let child_count = child_names.len();
        if choice <= child_count {
            node = &node.children[child_names[choice - 1]];
            continue;
        }
        let leaf_idx = choice - child_count - 1;
        if let Some(name) = leaf_names.get(leaf_idx) {
            return Ok(vendors_for_entry(name, vendors));
        }
        return Err("Invalid selection".into());
    }
}

/// Resolves a non-interactive `path` argument (e.g. `"Misc/Zariman/Cavalero"` or
/// `"zariman/cavalero"`, case-insensitive) by walking the tree segment-by-segment.
/// The final segment may name either a deeper directory (in which case everything
/// under it is returned) or a leaf entry (group/vendor name) directly.
fn resolve_path<'a>(
    root: &LocTree,
    path: &str,
    vendors: &'a [MappedVendor],
) -> AppResult<Vec<&'a MappedVendor>> {
    let mut node = root;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err("Empty vendor path".into());
    }

    for (i, segment) in segments.iter().enumerate() {
        if let Some(child) = node
            .children
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(segment))
        {
            node = child.1;
            continue;
        }
        // Not a directory segment — check whether it names a leaf entry. Only valid
        // as the *last* segment.
        if i == segments.len() - 1
            && let Some(name) = node
                .entries
                .iter()
                .find(|e| e.eq_ignore_ascii_case(segment))
        {
            return Ok(vendors_for_entry(name, vendors));
        }
        return Err(format!("No such vendor path segment: '{segment}'").into());
    }

    // Ran out of segments on a directory node — return everything under it.
    let mut names = std::collections::BTreeSet::new();
    all_entries(node, &mut names);
    let mut result = Vec::new();
    for name in names {
        result.extend(vendors_for_entry(&name, vendors));
    }
    Ok(result)
}

/// Prints the ranked-offering table for `rows`, reusing `cli::print_header` /
/// `cli::print_info` for visual consistency with the rest of the tool.
fn print_ranked_table(rows: &[RankedOffering]) {
    crate::cli::print_header("Vendor Rankings");
    if rows.is_empty() {
        tsprintln!("  No offerings met the demand-floor/saturation criteria.");
        return;
    }
    tsprintln!(
        "{:<28} | {:<32} | {:<10} | {:>10} | {:>8} | {:>10} | {}",
        "Vendor",
        "Offering",
        "Currency",
        "Cost",
        "Score",
        "Vol/day",
        "Note"
    );
    tsprintln!("{}", "-".repeat(120));
    for row in rows {
        let score_str = row
            .score
            .map_or_else(|| "—".to_string(), |s| format!("{s:.2}"));
        let note = row.note.as_deref().unwrap_or("");
        tsprintln!(
            "{:<28} | {:<32} | {:<10} | {:>10.1} | {:>8} | {:>10.1} | {}",
            row.vendor_name,
            row.offering_name,
            row.currency,
            row.amount,
            score_str,
            row.daily_volume,
            note
        );
    }
    crate::cli::print_info(
        "Saturation column",
        "not shown inline — check --max-saturation filtering",
    );
}

/// Writes `rows` as JSON to `vendor_rankings.json` in the project root (matching the
/// observed, if undocumented, location of `session_report.json`).
fn write_rankings_json(rows: &[RankedOffering]) -> AppResult<()> {
    let json = serde_json::to_string_pretty(rows)?;
    fs::write("vendor_rankings.json", json)?;
    tsprintln!("Wrote vendor_rankings.json ({} rows).", rows.len());
    Ok(())
}

/// Runs the vendor mode end-to-end: refreshes caches, builds the mapped vendor cache,
/// then either prints the D4 match-coverage report or resolves a location/vendor
/// selection (via `path`, or an interactive picker when `path` is `None`) and prints
/// the ranked-offering table for it.
///
/// # Errors
/// Returns an error if the cache refresh, vendor-cache build, or (for
/// non-interactive use) path resolution fails, or if `--write-json` can't write its
/// output file.
pub async fn run_vendor_cli(
    path: Option<&str>,
    match_report: bool,
    write_json: bool,
    max_saturation: Option<f64>,
) -> AppResult<()> {
    let vendors = VendorAnalysisService::load_vendors().await?;
    tsprintln!("Loaded {} vendors.", vendors.len());

    if match_report {
        print_match_report(&vendors);
        return Ok(());
    }

    let tree = build_location_tree(&vendors);
    let selected: Vec<&MappedVendor> = match path {
        Some(p) => resolve_path(&tree, p, &vendors)?,
        None => interactive_picker(&tree, &vendors)?,
    };

    if selected.is_empty() {
        tsprintln!("No vendors matched that selection.");
        return Ok(());
    }

    let owned: Vec<MappedVendor> = selected.into_iter().cloned().collect();
    let rows = VendorAnalysisService::rank(&owned, max_saturation).await?;
    print_ranked_table(&rows);

    if write_json {
        write_rankings_json(&rows)?;
    }

    Ok(())
}
