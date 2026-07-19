//! `--debug-mastery` checklist report.
//!
//! Extracted out of `main.rs` (Pass 1 mechanical refactor) — no behavior change.

use crate::mapping;
use crate::{tseprintln, tsprintln};

pub(crate) fn is_eligible_for_mastery_checklist(unique_name: &str) -> bool {
    !unique_name.starts_with("SolNode")
        && !unique_name.contains("/StoreItems/")
        && !unique_name.contains("PvPVariant")
        && !unique_name.contains("RewardItem")
        && !unique_name.contains("/Emotes/")
        && !unique_name.contains("Doppelganger") // exclude the fake Grimoire
}

/// Builds `uniqueName -> XP` from the save's `XPInfo` array.
///
/// `XPInfo` is the only reliable source for the reasons documented on
/// `load_mastery_and_ownership` — do not reintroduce a MechSuits/SpaceGuns-style override here.
fn build_xp_map(inventory: &serde_json::Value) -> std::collections::HashMap<String, u64> {
    let mut xp_map = std::collections::HashMap::new();
    if let Some(xp_info) = inventory.get("XPInfo").and_then(|v| v.as_array()) {
        for entry in xp_info {
            if let (Some(unique), Some(xp)) = (
                entry.get("ItemType").and_then(|v| v.as_str()),
                entry.get("XP").and_then(serde_json::Value::as_u64),
            ) {
                xp_map.insert(unique.to_string(), xp);
            }
        }
    }
    xp_map
}

/// Builds a filtered `lowercase display name -> uniqueName` lookup, preferring WFCD entries and
/// falling back to WFM's `game_ref` for anything WFCD doesn't cover.
fn build_name_to_unique_map(
    wfcd_by_ref: &std::collections::HashMap<String, crate::models::WfcdItem>,
    wfm_by_name: &std::collections::HashMap<String, crate::models::WfmItem>,
) -> std::collections::HashMap<String, String> {
    let mut name_to_unique = std::collections::HashMap::new();
    for (unique, item) in wfcd_by_ref {
        if is_eligible_for_mastery_checklist(unique) {
            let norm = item.name.to_lowercase();
            if let Some(existing) = name_to_unique.get(&norm) {
                tseprintln!(
                    "WARNING: Ambiguous display name '{}' maps to both '{}' and '{}' — picking first.",
                    item.name,
                    existing,
                    unique
                );
            } else {
                name_to_unique.insert(norm, unique.clone());
            }
        }
    }
    // WFM fallback (also filtered)
    for (name, item) in wfm_by_name {
        let norm = name.to_lowercase();
        if let Some(gr) = &item.game_ref
            && let Some(wfcd_item) = wfcd_by_ref.get(gr)
            && is_eligible_for_mastery_checklist(&wfcd_item.unique_name)
        {
            name_to_unique
                .entry(norm)
                .or_insert(wfcd_item.unique_name.clone());
        }
    }
    name_to_unique
}

/// Resolves and prints a single checklist line, given the already-built lookup tables. Falls
/// back to an `&`/`and` swap when the exact name isn't found, since wiki-sourced item names are
/// inconsistent about which spelling they use.
#[allow(clippy::too_many_arguments)]
fn print_checklist_row(
    name: &str,
    name_to_unique: &std::collections::HashMap<String, String>,
    xp_map: &std::collections::HashMap<String, u64>,
    wfcd_by_ref: &std::collections::HashMap<String, crate::models::WfcdItem>,
    wfm_by_ref: &std::collections::HashMap<String, crate::models::WfmItem>,
    mastered_set: &std::collections::HashSet<String>,
    frame_tier_uniques: &std::collections::HashSet<String>,
) {
    let norm = name.to_lowercase();
    let unique = name_to_unique.get(&norm).cloned().or_else(|| {
        if norm.contains('&') {
            let alt = norm.replace('&', "and");
            name_to_unique.get(&alt).cloned()
        } else {
            None
        }
    });

    let Some(unique) = unique else {
        tsprintln!("{name:<40} | Not found in WFCD/WFM");
        return;
    };

    let display_name = wfcd_by_ref.get(&unique).map_or("", |w| w.name.as_str());

    // Same classification + threshold logic the production mastery pass uses, so the debug
    // print can never disagree with the real keep/sell decisions.
    let required =
        mapping::mastery_threshold(display_name, &unique, frame_tier_uniques.contains(&unique));

    let max_rank = if let Some(wfm_item) = wfm_by_ref.get(&unique) {
        wfm_item.max_rank.unwrap_or(30)
    } else if let Some(wfcd) = wfcd_by_ref.get(&unique) {
        wfcd.fusion_limit
            .or_else(|| {
                wfcd.level_stats
                    .as_ref()
                    .map(|v| u32::try_from(v.len().saturating_sub(1)).unwrap_or(30))
            })
            .unwrap_or(30)
    } else {
        30
    };

    let xp = xp_map.get(&unique).copied().unwrap_or(0);
    let status = if xp == 0 {
        "No XP record"
    } else if xp >= required {
        "Mastered"
    } else {
        "Not Mastered"
    };
    let set_status = if mastered_set.contains(&unique) {
        "in set"
    } else {
        "not in set"
    };
    tsprintln!(
        "{name:<40} | {unique:<30} | {max_rank:>3}    | {required:>6} | {xp:>6} | {status:<10} ({set_status})"
    );
}

/// Runs the `--debug-mastery` checklist report: reads `config/mastery_checklist.txt` and prints
/// each listed item's resolved `uniqueName`, XP, mastery threshold, and Mastered/Not Mastered
/// status, for spot-checking the mastery logic against a real account.
///
/// # Errors
/// Returns an error if the checklist file exists but cannot be read.
pub(crate) fn run_debug_mastery_checklist(
    inventory: &serde_json::Value,
    wfcd_by_ref: &std::collections::HashMap<String, crate::models::WfcdItem>,
    wfm_by_ref: &std::collections::HashMap<String, crate::models::WfmItem>,
    wfm_by_name: &std::collections::HashMap<String, crate::models::WfmItem>,
    mastered_set: &std::collections::HashSet<String>,
    frame_tier_uniques: &std::collections::HashSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let xp_map = build_xp_map(inventory);
    let name_to_unique = build_name_to_unique_map(wfcd_by_ref, wfm_by_name);

    let checklist_path = "config/mastery_checklist.txt";
    if !std::path::Path::new(checklist_path).exists() {
        tseprintln!("Debug mastery checklist file not found: {checklist_path}");
        tseprintln!("Please create it with one item name per line.");
        return Ok(());
    }
    let content = std::fs::read_to_string(checklist_path)?;
    let items: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

    tsprintln!("\n=== Mastery Checklist Debug (with XP details) ===");
    tsprintln!(
        "{:<40} | {:<30} | MaxRank | Req XP | XP     | Status",
        "Item",
        "uniqueName"
    );
    tsprintln!("{}", "-".repeat(110));

    for name in items {
        print_checklist_row(
            name.trim(),
            &name_to_unique,
            &xp_map,
            wfcd_by_ref,
            wfm_by_ref,
            mastered_set,
            frame_tier_uniques,
        );
    }
    tsprintln!("=== End of Checklist ===\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoy_entries_are_excluded_from_checklist_matching() {
        assert!(!is_eligible_for_mastery_checklist("SolNode105"));
        assert!(!is_eligible_for_mastery_checklist(
            "/Lotus/Types/StoreItems/SuitCustomizations/ColourPickerJadeItem"
        ));
        assert!(!is_eligible_for_mastery_checklist(
            "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/PvPVariantTipOne"
        ));
        assert!(!is_eligible_for_mastery_checklist(
            "/Lotus/Types/Items/Deimos/WoundedInfestedPredatorUncommonRewardItem"
        ));
        assert!(is_eligible_for_mastery_checklist(
            "/Lotus/Powersuits/Fairy/Fairy"
        ));
    }

    #[test]
    fn doppelganger_decoy_is_excluded() {
        // Previously only excluded by the dead, test-only copy of this function — the nested
        // copy actually used by --debug-mastery was missing this check. Adjust the fixture
        // string below to a real Doppelganger uniqueName if you have one on hand.
        assert!(!is_eligible_for_mastery_checklist(
            "/Lotus/Types/Game/PlayerCustomizations/Doppelganger/SomeFakeGrimoire"
        ));
    }
}
