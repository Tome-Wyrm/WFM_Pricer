// src/vendor.rs -> src/vendor/mod.rs
//! Vendor pipeline: wiki Lua fetch/parse → `vendors.toml` overlay/matching →
//! cost/score ranking → CLI presentation. Split into submodules by pipeline stage;
//! this file just wires them together and re-exports the original flat `vendor::*`
//! surface so call sites elsewhere in the crate don't need to change.
//!
//! - `lua`: minimal Lua-subset tokenizer/parser for `Module:Vendors/data`.
//! - `raw`: raw vendor/offering parsing + `vendors_raw_cache.json`.
//! - `metadata`: `vendors.toml` overlay, category tradeability, name normalization.
//! - `matching`: raw + metadata + WFM slug matching -> `MappedVendor`/`MappedOffering`.
//! - `scoring`: cost classification + score/rank/filter pipeline.
//! - `network`: wiki fetch (revid-cached) + parse orchestration.
//! - `interactive`: location picker, ranked-table printing, `run_vendor_cli`.

mod interactive;
mod lua;
mod matching;
mod metadata;
mod network;
mod raw;
mod scoring;

pub use interactive::run_vendor_cli;
pub use lua::{LuaKey, LuaValue, RankList, Token, parse, tokenize};
pub use matching::{
    MappedOffering, MappedVendor, VendorMatchStats, build_and_write_vendor_cache,
    compute_match_stats, dump_category_audit, print_match_report,
};
pub use metadata::{
    CategoryClass, CostMode, VendorConfig, VendorMeta, classify_category, is_tradeable_category,
    load_vendor_metadata, normalize_item_name,
};
pub use network::{
    fetch_and_cache_vendors, fetch_latest_revid, fetch_vendors_lua, parse_vendors_from_lua,
    read_cached_revid, write_cached_revid,
};
pub use raw::{
    CurrencySpec, PrereqSpec, PriceSpec, RawOffering, RawVendor, load_vendor_data,
    normalize_category, parse_raw_offering, parse_raw_vendor, write_vendor_cache,
};
pub use scoring::{
    Cost, CostRow, RankedOffering, ScoreInput, ScoredRow, classify_cost, classify_vendor_offerings,
    cost_rows, exceeds_saturation_cap, rank_by_score, rank_offerings, unclassified_offerings,
};

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::config;
    use crate::tseprintln;

    #[test]
    fn parse_sample_vendor_fixture() {
        // Exercises the full tokenize → parse pipeline against a representative
        // Acrithis-shaped snippet (same structure as the real wiki data).
        // The old version of this test called extract_lua_content() on a v1-format
        // JSON fixture; that function has been removed now that the fetch path uses
        // the v2 API and returns plain Lua. The fixture was only testing the
        // tokenizer/parser, so we inline equivalent Lua here instead.
        let lua_source = r#"
return {
    Vendors = {
        Acrithis = {
            Currency = "Pathos Clamp",
            Name = "Acrithis",
            Type = "Store",
            Offerings = {
                { "Orokin Reactor", "Item", 20, 1, Prereq = 0 },
                { "Orokin Catalyst", "Item", 20, 1, Prereq = 0 },
            }
        }
    }
}
        "#;
        let tokens = tokenize(lua_source).expect("Tokenizer failed");
        let parsed = parse(&tokens).expect("Parser failed");

        match parsed {
            LuaValue::Table(outer) => {
                let vendors_opt = outer.iter().find_map(|(key, val)| {
                    if let Some(LuaKey::String(name)) = key {
                        if name == "Vendors" { Some(val) } else { None }
                    } else {
                        None
                    }
                });
                let vendors_table = vendors_opt
                    .expect("No Vendors key found")
                    .as_table()
                    .expect("Vendors is not a table");

                let acrithis_opt = vendors_table.iter().find_map(|(key, val)| {
                    if let Some(LuaKey::String(name)) = key {
                        if name == "Acrithis" {
                            Some(val.as_table().expect("Acrithis not a table"))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                let acrithis = acrithis_opt.expect("Acrithis not found");
                let currency_opt = acrithis.iter().find_map(|(key, val)| {
                    if let Some(LuaKey::String(name)) = key {
                        if name == "Currency" {
                            Some(val.as_str().expect("Currency not a string"))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                assert_eq!(currency_opt, Some("Pathos Clamp"));

                let offerings_opt = acrithis.iter().find_map(|(key, val)| {
                    if let Some(LuaKey::String(name)) = key {
                        if name == "Offerings" {
                            Some(val.as_table().expect("Offerings not a table"))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                let offerings = offerings_opt.expect("Offerings not found");
                assert_eq!(offerings.len(), 2);
            }
            _ => panic!("Top-level is not a table"),
        }
    }

    #[test]
    fn parse_full_vendors_cache() {
        let cache_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        if !cache_path.exists() {
            tseprintln!(
                "Skipping full cache test – cache file not found. Run the fetch step first."
            );
            return;
        }
        let vendors = load_vendor_data().expect("Failed to load vendor data");
        assert!(
            vendors.len() >= 60,
            "Expected at least 60 vendors, got {}",
            vendors.len()
        );
        let has_simaris = vendors.iter().any(|v| v.key == "Cephalon Simaris");
        assert!(has_simaris, "Cephalon Simaris not found");
    }

    /// C2 tripwire: every vendor key in the raw cache must appear in vendors.toml as either
    /// a located vendor or an explicitly excluded one.  Fails loudly when a wiki update
    /// adds a new vendor that hasn't been triaged yet.
    #[test]
    fn all_cached_vendors_have_toml_entry() {
        let cache_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        let config_path = std::path::Path::new(config::VENDORS_CONFIG_FILE);
        if !cache_path.exists() || !config_path.exists() {
            tseprintln!("Skipping tripwire – cache or config file not found.");
            return;
        }
        let vendors = load_vendor_data().expect("Failed to load vendor data");
        let meta = load_vendor_metadata().expect("Failed to load vendor metadata");

        let mut missing = Vec::new();
        for v in &vendors {
            match meta.get(&v.key) {
                Some(m) if m.excluded || m.location.is_some() => {}
                Some(_) => missing.push(format!(
                    "{} (has entry but no location and not excluded)",
                    v.key
                )),
                None => missing.push(format!("{} (not in vendors.toml at all)", v.key)),
            }
        }
        assert!(
            missing.is_empty(),
            "Vendors in cache but not fully triaged in vendors.toml:\n  {}",
            missing.join("\n  ")
        );
    }

    /// C3 pool consistency check: vendors that share a group name must all agree
    /// on that name (guards against typos between entries in vendors.toml).
    #[test]
    fn pool_group_members_are_internally_consistent() {
        // Expected pool → member keys, derived from the planning doc.
        // Update this list if a pool changes.
        let expected_pools: &[(&str, &[&str])] = &[
            (
                "Ostron",
                &[
                    "Fisher Hai-Luk",
                    "Hok",
                    "Master Teasonai",
                    "Old Man Suumbaat",
                ],
            ),
            (
                "Solaris United",
                &["Legs", "Rude Zuud", "Smokefinger", "The Business"],
            ),
            ("Entrati", &["Daughter", "Father", "Otak", "Son"]),
            (
                "The Hex",
                &["Amir", "Aoi", "Quincy", "Minerva", "Velimir", "Eleanor"],
            ),
            ("The Holdfasts", &["Cavalero", "Hombask"]),
        ];

        let config_path = std::path::Path::new(config::VENDORS_CONFIG_FILE);
        if !config_path.exists() {
            tseprintln!("Skipping pool consistency check – vendors.toml not found.");
            return;
        }
        let meta = load_vendor_metadata().expect("Failed to load vendor metadata");

        for (pool, members) in expected_pools {
            for member in *members {
                let entry = meta.get(*member);
                match entry {
                    Some(m) => assert_eq!(
                        m.group.as_deref(),
                        Some(*pool),
                        "Vendor '{}' should have group '{}' but has {:?}",
                        member,
                        pool,
                        m.group
                    ),
                    None => panic!(
                        "Pool member '{}' (group '{}') is missing from vendors.toml",
                        member, pool
                    ),
                }
            }
        }
    }
}
