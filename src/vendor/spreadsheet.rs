//! Reference Data Ingestion & Spreadsheet Parser Pipeline.
//!
//! Parses CSV/TSV spreadsheet data for human-curated vendor reference items and
//! validates entries against WFCD/WFM catalog rules before writing into SQLite `reference.db`.

use crate::AppResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A row in the human-curated vendor reference spreadsheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorSpreadsheetRow {
    pub vendor_key: String,
    pub vendor_name: String,
    pub location: Option<String>,
    pub group: Option<String>,
    pub offering_name: String,
    pub category: String,
    pub currency: String,
    pub cost_amount: f64,
    pub cost_mode: String, // "Single", "AnyOf", "AllOf"
    pub wfm_slug: Option<String>,
    pub target_rank: Option<u32>,
    pub notes: Option<String>,
}

/// Validation result for a single spreadsheet row.
#[derive(Debug, Clone)]
pub enum SpreadsheetRowValidationError {
    EmptyVendorKey,
    EmptyOfferingName,
    InvalidCostAmount(f64),
    InvalidCostMode(String),
    UnknownWfmSlug(String),
}

/// Parses a CSV string containing curated vendor offerings into structured rows.
///
/// # Errors
/// Returns an error if CSV formatting is invalid.
pub fn parse_vendor_csv(csv_content: &str) -> AppResult<Vec<VendorSpreadsheetRow>> {
    let mut rows = Vec::new();
    let mut lines = csv_content.lines();

    // Skip header line if present
    let first_line = match lines.next() {
        Some(line) => line,
        None => return Ok(rows),
    };

    let has_header = first_line.to_lowercase().contains("vendor_key")
        || first_line.to_lowercase().contains("vendor");

    let lines_to_parse: Vec<&str> = if has_header {
        lines.collect()
    } else {
        std::iter::once(first_line).chain(lines).collect()
    };

    for line in lines_to_parse {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
        if parts.len() < 7 {
            continue;
        }

        let row = VendorSpreadsheetRow {
            vendor_key: parts[0].to_string(),
            vendor_name: parts[1].to_string(),
            location: if parts.len() > 2 && !parts[2].is_empty() { Some(parts[2].to_string()) } else { None },
            group: if parts.len() > 3 && !parts[3].is_empty() { Some(parts[3].to_string()) } else { None },
            offering_name: parts[4].to_string(),
            category: parts[5].to_string(),
            currency: parts[6].to_string(),
            cost_amount: parts.get(7).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0),
            cost_mode: parts.get(8).unwrap_or(&"Single").to_string(),
            wfm_slug: parts.get(9).filter(|s| !s.is_empty()).map(|s| (*s).to_string()),
            target_rank: parts.get(10).and_then(|s| s.parse::<u32>().ok()),
            notes: parts.get(11).filter(|s| !s.is_empty()).map(|s| (*s).to_string()),
        };

        rows.push(row);
    }

    Ok(rows)
}

/// Validates spreadsheet rows against an optional WFM slug map.
pub fn validate_spreadsheet_rows(
    rows: &[VendorSpreadsheetRow],
    wfm_slugs: Option<&HashMap<String, String>>,
) -> Vec<(usize, SpreadsheetRowValidationError)> {
    let mut errors = Vec::new();

    for (idx, row) in rows.iter().enumerate() {
        if row.vendor_key.is_empty() {
            errors.push((idx, SpreadsheetRowValidationError::EmptyVendorKey));
        }
        if row.offering_name.is_empty() {
            errors.push((idx, SpreadsheetRowValidationError::EmptyOfferingName));
        }
        if row.cost_amount <= 0.0 {
            errors.push((idx, SpreadsheetRowValidationError::InvalidCostAmount(row.cost_amount)));
        }
        if !["Single", "AnyOf", "AllOf"].contains(&row.cost_mode.as_str()) {
            errors.push((idx, SpreadsheetRowValidationError::InvalidCostMode(row.cost_mode.clone())));
        }
        if let Some(slug) = &row.wfm_slug
            && let Some(slug_map) = wfm_slugs
            && !slug_map.contains_key(slug)
        {
            errors.push((idx, SpreadsheetRowValidationError::UnknownWfmSlug(slug.clone())));
        }
    }

    errors
}
