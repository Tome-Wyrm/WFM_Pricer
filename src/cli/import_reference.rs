//! `import-reference` CLI command (Architecture Evolution Plan, Phase 2, item 1: the
//! Spreadsheet Ingestion Pipeline). `vendor::spreadsheet::{parse_vendor_csv,
//! validate_spreadsheet_rows}` and `ReferenceRepositorySqlite::import_spreadsheet_rows`
//! already existed but nothing called them end-to-end — this is that missing entry
//! point: Spreadsheet Source Data -> Validation -> Importer -> `reference.db`.

use crate::AppResult;
use crate::repository::ReferenceRepositorySqlite;
use crate::vendor::spreadsheet::{
    SpreadsheetRowValidationError, parse_vendor_csv, parse_vendor_tsv, validate_spreadsheet_rows,
};
use std::path::Path;

use super::{print_header, print_warning, tseprintln, tsprintln};

/// Parses, validates, and (unless `dry_run`) imports a curated vendor-offering
/// spreadsheet into `reference.db`'s `curated_vendor_offerings` table.
///
/// # Errors
/// Returns an error if the file can't be read, isn't valid CSV/TSV for the expected
/// `VendorSpreadsheetRow` shape, or (on a real, non-dry-run import) `reference.db`
/// can't be opened or written to. Row-level validation problems (empty fields, bad
/// cost_mode, etc.) are reported and abort the import, but are not themselves a
/// returned `Err` — this always exits cleanly once it's told the user what's wrong.
pub async fn run_import_reference_cli(path: &Path, dry_run: bool) -> AppResult<()> {
    print_header("Reference Spreadsheet Import");

    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Could not read '{}': {e}", path.display()))?;

    let is_tsv = path.extension().and_then(|e| e.to_str()) == Some("tsv");
    let rows = if is_tsv {
        parse_vendor_tsv(&content)
    } else {
        parse_vendor_csv(&content)
    }?;

    tsprintln!("Parsed {} row(s) from {}.", rows.len(), path.display());

    if rows.is_empty() {
        print_warning(
            "No rows parsed. Expected a header row with at least vendor_key, vendor_name, \
             offering_name, category, currency, cost_amount (optional: location, group, \
             cost_mode, wfm_slug, target_rank, notes).",
        );
        return Ok(());
    }

    // No wired-up WFCD/WFM item catalog to cross-check wfm_slug against yet, so that
    // check is skipped (None) rather than flagging every populated wfm_slug as unknown.
    let validation_errors = validate_spreadsheet_rows(&rows, None);

    if validation_errors.is_empty() {
        tsprintln!("All {} row(s) passed validation.", rows.len());
    } else {
        print_warning(&format!(
            "{} validation issue(s) found:",
            validation_errors.len()
        ));
        for (idx, err) in &validation_errors {
            let line_num = idx + 2; // account for header line
            tseprintln!("  Line {line_num}: {}", describe_error(err));
        }
    }

    let blocking_errors = validation_errors
        .iter()
        .filter(|(_, e)| !matches!(e, SpreadsheetRowValidationError::UnknownWfmSlug(_)))
        .count();

    if blocking_errors > 0 {
        print_warning(&format!(
            "{blocking_errors} row(s) have blocking errors (empty vendor_key/offering_name, \
             cost_amount <= 0, or invalid cost_mode) — import aborted. Fix the spreadsheet and \
             re-run."
        ));
        return Ok(());
    }

    if dry_run {
        tsprintln!("--dry-run set: validated only, nothing written to reference.db.");
        return Ok(());
    }

    let mut repo = ReferenceRepositorySqlite::<()>::open_default()
        .map_err(|e| format!("Could not open reference.db: {e}"))?;
    let count = repo
        .import_spreadsheet_rows(&rows)
        .map_err(|e| format!("Import failed: {e}"))?;

    tsprintln!("Imported {count} row(s) into reference.db (curated_vendor_offerings table).");
    tsprintln!(
        "Note: this replaces the entire table contents — re-running with a smaller sheet drops \
         rows that aren't in it."
    );

    Ok(())
}

fn describe_error(err: &SpreadsheetRowValidationError) -> String {
    match err {
        SpreadsheetRowValidationError::EmptyVendorKey => "empty vendor_key".to_string(),
        SpreadsheetRowValidationError::EmptyOfferingName => "empty offering_name".to_string(),
        SpreadsheetRowValidationError::InvalidCostAmount(v) => {
            format!("invalid cost_amount ({v}) — must be greater than 0")
        }
        SpreadsheetRowValidationError::InvalidCostMode(m) => {
            format!("invalid cost_mode '{m}' — must be Single, AnyOf, or AllOf")
        }
        SpreadsheetRowValidationError::UnknownWfmSlug(s) => {
            format!(
                "unrecognized wfm_slug '{s}' (warning only, not blocking — no item catalog wired up yet)"
            )
        }
    }
}
