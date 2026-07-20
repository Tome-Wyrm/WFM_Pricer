use crate::AppResult;
use std::fs;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionReportItem {
    pub name: String,
    pub slug: String,
    pub price: u32,
    pub quantity: u32,
    pub rank: Option<u32>,
    pub action: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionReport {
    pub timestamp: String,
    pub username: String,
    pub items_processed: Vec<SessionReportItem>,
}

/// Writes the session report to `session_report.json`.
pub(crate) fn write_session_report(
    report: &SessionReport,
) -> AppResult<()> {
    let content = serde_json::to_string_pretty(report)?;
    fs::write("session_report.json", content)?;
    Ok(())
}
