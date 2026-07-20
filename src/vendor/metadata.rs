// src/vendor/metadata.rs
//! `config/vendors.toml` overlay: per-vendor metadata, category tradeability, and item
//! name normalization (Phase C/D).
use serde::{Deserialize, Serialize};
use crate::AppResult;
use std::fs;
use std::path::Path;

// ---- Vendor metadata (config/vendors.toml) ----

/// How multiple currencies on a single offering should be interpreted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CostMode {
    /// Single currency (default — price is a plain number using the vendor's currency).
    #[default]
    Single,
    /// Pay with any one of the listed currencies (buyer's choice).
    AnyOf,
    /// All listed currencies are required simultaneously.
    AllOf,
}

/// Per-vendor metadata from `config/vendors.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VendorMeta {
    /// Nav-tree location string, e.g. `"Misc/Zariman"`. Required unless `excluded = true`.
    #[serde(default)]
    pub location: Option<String>,
    /// Shared standing pool name, e.g. `"Ostron"`. Absent means standalone.
    #[serde(default)]
    pub group: Option<String>,
    /// If true, never surface this vendor in rankings or the picker.
    #[serde(default)]
    pub excluded: bool,
    /// How multi-currency prices on this vendor's offerings should be classified.
    #[serde(default)]
    pub cost_mode: CostMode,
    /// True for vendors whose wares are hand-entered rather than auto-parsed.
    #[serde(default)]
    pub hand_curated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VendorConfig {
    #[serde(default)]
    vendor: std::collections::HashMap<String, VendorMeta>,
}

/// Loads `config/vendors.toml` into a map of vendor key → `VendorMeta`.
/// Returns an empty map (not an error) if the file does not exist yet.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_vendor_metadata()
-> AppResult<std::collections::HashMap<String, VendorMeta>> {
    let path = Path::new(crate::config::VENDORS_CONFIG_FILE);
    if !path.exists() {
        return Ok(std::collections::HashMap::new());
    }
    let content = fs::read_to_string(path)?;
    let config: VendorConfig =
        toml::from_str(&content).map_err(|e| format!("Failed to parse vendors.toml: {e}"))?;
    Ok(config.vendor)
}

// ================== Phase D: WFM slug mapping + match-coverage report ==================

// ---- D1: category allowlist ----

/// Categories confirmed tradeable on Warframe.market. Kept as a real allowlist (not
/// "everything except X") so a brand-new category from a future wiki update fails
/// closed — see `classify_category`'s tripwire test (`all_cached_categories_are_classified`)
/// rather than silently being treated as tradeable or silently mismatched.
const TRADEABLE_CATEGORIES: &[&str] = &[
    "Arcane",
    "Ayatan Sculpture",
    "Blueprint",
    "Captura Scene",
    "Captura",
    "Cosmetic",
    "Emote",
    "Item",
    "Key",
    "Mod",
    "Relic",
    "Resource",
    "Riven Mod",
    "Riven",
    "Scene",
    "Weapon",
];
/// Categories seen in the wiki dump that are known and explicitly *not* tradeable.
/// Listed out (rather than inferred by "not in the allowlist") so `classify_category`
/// can distinguish "known and excluded" from "never triaged" — only the latter is the
/// tripwire case.
const NON_TRADEABLE_CATEGORIES: &[&str] = &[
    "Color",
    "Credits",
    "Decoration",
    "Emblem",
    "Ephemera",
    "Gear",
    "Glyph",
    "Honoria",
    "Landing Craft",
    "Misc",
    "Sigil",
    "Signa",
    "Somachord",
    "Sugatra",
    "Syandana",
    "Warframe",
];

/// Whether `category` (already run through `normalize_category`) is tradeable on WFM.
#[must_use]
pub fn is_tradeable_category(category: &str) -> bool {
    TRADEABLE_CATEGORIES.contains(&category)
}

/// Result of triaging a category against the D1 allow/deny lists. `Unknown` is the
/// tripwire case — a category the wiki dump contains that hasn't been explicitly
/// sorted into either list yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryClass {
    Tradeable,
    NonTradeable,
    Unknown,
}

#[must_use]
pub fn classify_category(category: &str) -> CategoryClass {
    if TRADEABLE_CATEGORIES.contains(&category) {
        CategoryClass::Tradeable
    } else if NON_TRADEABLE_CATEGORIES.contains(&category) {
        CategoryClass::NonTradeable
    } else {
        CategoryClass::Unknown
    }
}

// ---- D2: item name normalizer ----

/// If `s` starts with an `x`/`X`, returns the remainder of the string after it;
/// otherwise `None`. Helper for `normalize_item_name`.
fn strip_x_prefix(s: &str) -> Option<&str> {
    let mut chars = s.chars();
    match chars.next() {
        Some('x' | 'X') => Some(chars.as_str()),
        _ => None,
    }
}

/// Strips a yield-multiplier token (`"x10"`, `"(x20)"`, `"X 10"`) out of a raw vendor
/// offering name so it matches WFM's plain `"<Name> Blueprint"` listing. This only
/// handles multiplier removal — `mapping::find_wfm_match`'s lowercase/"set"-suffix
/// normalization still runs downstream on the result, so the two aren't duplicated.
#[must_use]
pub fn normalize_item_name(raw: &str) -> String {
    let words: Vec<&str> = raw.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let core = word.trim_start_matches('(');
        if let Some(rest) = strip_x_prefix(core) {
            let rest_trimmed = rest.trim_end_matches(')');
            if rest_trimmed.is_empty() {
                // Bare "x"/"X" (optionally "(x") — the count may be the next word,
                // e.g. "Vapor Specter X 10 Blueprint".
                if let Some(next) = words.get(i + 1) {
                    let next_core = next.trim_end_matches(')');
                    if !next_core.is_empty() && next_core.chars().all(|c| c.is_ascii_digit()) {
                        i += 2; // consume both the "x"/"X" token and the count
                        continue;
                    }
                }
                out.push(word.to_string());
            } else if rest_trimmed.chars().all(|c| c.is_ascii_digit()) {
                // Self-contained multiplier token: "x10", "(x20)", "x20)".
                i += 1;
                continue;
            } else {
                // Starts with x/X but isn't a multiplier (e.g. "Xoris") — keep as-is.
                out.push(word.to_string());
            }
        } else {
            out.push(word.to_string());
        }
        i += 1;
    }
    out.join(" ")
}

// ---- D3: slug matcher + rank targeting ----

/// `Some(0)` for categories where the vendor always sells the unranked/base copy
/// (Mod, Arcane); `None` for everything else, per the plan.
#[must_use]
pub(crate) fn target_rank_for(category: &str) -> Option<u32> {
    match category {
        "Mod" | "Arcane" => Some(0),
        _ => None,
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    /// C1: loader handles all field types including defaults.
    #[test]
    fn load_vendor_metadata_parses_all_fields() {
        let toml_str = r#"
[vendor."Acrithis"]
location = "Misc/Zariman"

[vendor."Cavalero"]
location = "Misc/Zariman"
group = "The Holdfasts"

[vendor."Conclave"]
excluded = true

[vendor."Unearth Citrine"]
location = "Deimos/SanctumAnotomica"
cost_mode = "all_of"

[vendor."Operational Supply"]
location = "Earth/Cetus"
cost_mode = "any_of"

[vendor."Marie"]
location = "Deimos/SanctumAnotomica"
hand_curated = true
"#;
        let config: super::VendorConfig = toml::from_str(toml_str).expect("TOML parse failed");
        let meta = config.vendor;

        // Defaults
        let acrithis = meta.get("Acrithis").expect("Acrithis missing");
        assert_eq!(acrithis.location.as_deref(), Some("Misc/Zariman"));
        assert!(acrithis.group.is_none());
        assert!(!acrithis.excluded);
        assert_eq!(acrithis.cost_mode, CostMode::Single);
        assert!(!acrithis.hand_curated);

        // Group
        let cavalero = meta.get("Cavalero").expect("Cavalero missing");
        assert_eq!(cavalero.group.as_deref(), Some("The Holdfasts"));

        // Excluded (no location required)
        let conclave = meta.get("Conclave").expect("Conclave missing");
        assert!(conclave.excluded);

        // cost_mode variants
        let citrine = meta
            .get("Unearth Citrine")
            .expect("Unearth Citrine missing");
        assert_eq!(citrine.cost_mode, CostMode::AllOf);
        let supply = meta
            .get("Operational Supply")
            .expect("Operational Supply missing");
        assert_eq!(supply.cost_mode, CostMode::AnyOf);

        // hand_curated
        let marie = meta.get("Marie").expect("Marie missing");
        assert!(marie.hand_curated);
    }
}

#[cfg(test)]

mod category_tests {
    use super::*;
    use crate::config;
    use crate::tseprintln;
    use crate::vendor::raw::load_vendor_data;

    #[test]
    fn classify_category_known_cases() {
        assert_eq!(classify_category("Mod"), CategoryClass::Tradeable);
        assert_eq!(classify_category("Captura Scene"), CategoryClass::Tradeable);
        assert_eq!(classify_category("Sigil"), CategoryClass::NonTradeable);
        assert_eq!(classify_category("Glyph"), CategoryClass::NonTradeable);
        assert_eq!(
            classify_category("SomeBrandNewCategory"),
            CategoryClass::Unknown
        );
    }

    #[test]
    fn is_tradeable_category_matches_classify() {
        assert!(is_tradeable_category("Weapon"));
        assert!(!is_tradeable_category("Glyph"));
        assert!(!is_tradeable_category("SomeBrandNewCategory"));
    }

    /// D1 tripwire: every category seen across the real vendor cache must be
    /// explicitly triaged into `TRADEABLE_CATEGORIES` or `NON_TRADEABLE_CATEGORIES`.
    /// Same pattern as C2's `all_cached_vendors_have_toml_entry` — fails loudly when a
    /// wiki update introduces a category nobody's looked at yet.
    #[test]
    fn all_cached_categories_are_classified() {
        let cache_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        if !cache_path.exists() {
            tseprintln!("Skipping category tripwire – cache file not found.");
            return;
        }
        let vendors = load_vendor_data().expect("Failed to load vendor data");
        let mut unknown: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for v in &vendors {
            for o in &v.offerings {
                if classify_category(&o.category) == CategoryClass::Unknown {
                    unknown.insert(o.category.clone());
                }
            }
        }
        assert!(
            unknown.is_empty(),
            "Unclassified categories found in vendor cache — triage into \
             TRADEABLE_CATEGORIES or NON_TRADEABLE_CATEGORIES in vendor.rs:\n  {}",
            unknown.into_iter().collect::<Vec<_>>().join("\n  ")
        );
    }
}

#[cfg(test)]

mod name_normalizer_tests {
    use super::*;

    #[test]
    fn normalize_item_name_strips_yield_multipliers() {
        let cases = [
            ("Tear Azurite x10 Blueprint", "Tear Azurite Blueprint"),
            ("Fosfor Blau (x20) Blueprint", "Fosfor Blau Blueprint"),
            ("Vapor Specter X 10 Blueprint", "Vapor Specter Blueprint"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_item_name(input), expected, "input: {input}");
        }
    }

    #[test]
    fn normalize_item_name_leaves_ordinary_names_alone() {
        assert_eq!(normalize_item_name("Orokin Reactor"), "Orokin Reactor");
        // Starts with "X" but isn't a multiplier — must not be mangled.
        assert_eq!(normalize_item_name("Xoris Blueprint"), "Xoris Blueprint");
    }
}
