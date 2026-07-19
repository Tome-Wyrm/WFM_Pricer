//! Loading the WFCD/WFM cache files into in-memory lookup tables, and matching a WFCD item's
//! display name against the WFM name index.

use std::collections::HashMap;
use std::error::Error;
use std::fs;

use crate::config::{WFCD_CACHE_FILE, WFM_CACHE_FILE};
use crate::models::{WfcdItem, WfmItem, WfmV2Response};

// ── Type aliases for complex return types ──────────────────────────────────

type LookupTables = (
    HashMap<String, WfcdItem>,
    HashMap<String, WfmItem>,
    HashMap<String, WfmItem>,
    HashMap<String, WfmItem>,
);

pub(crate) fn find_wfm_match<'a>(
    name: &str,
    wfm_by_name: &'a HashMap<String, WfmItem>,
) -> Option<&'a WfmItem> {
    let lower_name = name.to_lowercase();

    if let Some(item) = wfm_by_name.get(&lower_name) {
        return Some(item);
    }

    if lower_name.ends_with(" set") {
        let stripped = &lower_name[..lower_name.len() - 4];
        if let Some(item) = wfm_by_name.get(stripped) {
            return Some(item);
        }
    }

    None
}

pub(crate) fn load_lookup_tables() -> Result<LookupTables, Box<dyn Error>> {
    let wfcd_str = fs::read_to_string(WFCD_CACHE_FILE)?;
    let wfcd_items: Vec<WfcdItem> = serde_json::from_str(&wfcd_str)
        .map_err(|e| format!("Failed to parse cached WFCD All.json: {e:?}"))?;

    let wfm_str = fs::read_to_string(WFM_CACHE_FILE)?;
    let wfm_response: WfmV2Response = serde_json::from_str(&wfm_str)
        .map_err(|e| format!("Failed to parse cached WFM v2 items list: {e:?}"))?;

    let mut wfcd_by_ref = HashMap::new();
    for item in wfcd_items {
        wfcd_by_ref.insert(item.unique_name.clone(), item);
    }

    let mut wfm_by_ref = HashMap::new();
    let mut wfm_by_name = HashMap::new();
    let mut wfm_by_slug = HashMap::new();
    for item in wfm_response.data {
        if let Some(ref gr) = item.game_ref {
            wfm_by_ref.insert(gr.clone(), item.clone());
        }
        wfm_by_name.insert(item.i18n.en.name.to_lowercase(), item.clone());
        wfm_by_slug.insert(item.slug.clone(), item);
    }

    Ok((wfcd_by_ref, wfm_by_ref, wfm_by_name, wfm_by_slug))
}
