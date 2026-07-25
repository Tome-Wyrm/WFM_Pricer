//! Spreadsheet Ingestion Pipeline — Parses and validates user-curated CSV/TSV spreadsheets
//! into structured vendor offerings, game items, and standing currency systems for `reference.db`.

use crate::AppResult;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Represents a single vendor offering parsed from a reference CSV/TSV file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpreadsheetVendorOffering {
    pub vendor_name: String,
    pub vendor_group: Option<String>,
    pub location: Option<String>,
    pub item_name: String,
    pub item_slug: String,
    pub standing_cost: u32,
    pub item_cost_currency: Option<String>,
    pub item_cost_quantity: Option<u32>,
    pub cost_type: String, // "Single", "AnyOf", "AllOf"
}

/// Validation result reporting errors or warnings found during ingestion.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Parses a CSV/TSV spreadsheet file into raw records.
pub fn parse_spreadsheet_file<P: AsRef<Path>>(
    path: P,
) -> AppResult<Vec<SpreadsheetVendorOffering>> {
    let path = path.as_ref();
    let content = fs::read_to_string(path)?;
    let delimiter = if path.extension().and_then(|e| e.to_str()) == Some("tsv") {
        b'\t'
    } else {
        b','
    };

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .delimiter(delimiter)
        .trim(csv::Trim::All)
        .from_reader(content.as_bytes());

    let mut offerings = Vec::new();
    for result in rdr.deserialize() {
        let record: SpreadsheetVendorOffering =
            result.map_err(|e| format!("CSV Parse Error: {e}"))?;
        offerings.push(record);
    }

    Ok(offerings)
}

/// Validates parsed spreadsheet records against rules (no empty item names, duplicate check, invalid cost types).
pub fn validate_spreadsheet_offerings(offerings: &[SpreadsheetVendorOffering]) -> ValidationResult {
    let mut validation = ValidationResult::default();
    let mut seen_keys = HashSet::new();

    for (idx, offering) in offerings.iter().enumerate() {
        let line_num = idx + 2; // account for header line

        if offering.vendor_name.trim().is_empty() {
            validation
                .errors
                .push(format!("Line {line_num}: Empty vendor_name"));
        }
        if offering.item_name.trim().is_empty() {
            validation
                .errors
                .push(format!("Line {line_num}: Empty item_name"));
        }
        if offering.item_slug.trim().is_empty() {
            validation
                .errors
                .push(format!("Line {line_num}: Empty item_slug"));
        }

        let cost_type = offering.cost_type.to_uppercase();
        if !["SINGLE", "ANYOF", "ALLOF"].contains(&cost_type.as_str()) {
            validation.errors.push(format!(
                "Line {line_num}: Invalid cost_type '{}'. Must be Single, AnyOf, or AllOf.",
                offering.cost_type
            ));
        }

        let unique_key = format!(
            "{}:{}:{}",
            offering.vendor_name.to_lowercase(),
            offering.item_slug.to_lowercase(),
            offering.standing_cost
        );
        if !seen_keys.insert(unique_key) {
            validation.warnings.push(format!(
                "Line {line_num}: Duplicate offering detected for vendor '{}' and item '{}'",
                offering.vendor_name, offering.item_slug
            ));
        }
    }

    validation
}
