// src/vendor/raw.rs
//! Raw (unmapped) vendor/offering parsing straight out of the tokenized Lua, plus the
//! `vendors_raw_cache.json` read/write.
use super::lua::{LuaKey, LuaValue, RankList, parse_rank_list};
use crate::config;
use crate::tseprintln;
use serde::{Deserialize, Serialize};

// ---------- Load Vendor Data ----------

/// Loads the parsed vendor data from the cached file.
///
/// # Errors
/// Returns a `String` error if the file cannot be read or the JSON is malformed.
pub fn load_vendor_data() -> Result<Vec<RawVendor>, String> {
    let path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
    if !path.exists() {
        return Err(format!("Vendor cache file not found: {}", path.display()));
    }
    let json =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read vendor cache: {e}"))?;
    let vendors: Vec<RawVendor> = serde_json::from_str(&json)
        .map_err(|e| format!("Failed to parse vendor cache JSON: {e}"))?;
    Ok(vendors)
}

/// Writes a vector of `RawVendor` to the cache file.
///
/// # Errors
/// Returns a `String` error if serialization or file write fails.
pub fn write_vendor_cache(vendors: &[RawVendor]) -> Result<(), String> {
    let path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
    let json = serde_json::to_string_pretty(vendors)
        .map_err(|e| format!("Failed to serialize vendor cache: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write vendor cache: {e}"))?;
    Ok(())
}

// ========== Offering Normalizer ==========

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PriceSpec {
    Single(String, f64),
    Multi(Vec<(String, f64)>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrereqSpec {
    Rank(u32),
    Quest(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawOffering {
    pub name: String,
    pub category: String,
    pub price: PriceSpec,
    pub qty: u32,
    pub prereq: Option<PrereqSpec>,
    pub timer: Option<u64>,
    pub limit: Option<u32>,
}

/// Normalizes known category typos from the data.
/// Only modifies specific known issues; otherwise leaves the string as-is.
#[must_use]
pub fn normalize_category(cat: &str) -> String {
    let trimmed = cat.trim();
    match trimmed {
        "Resource," => "Resource".to_string(),
        "Cosmetics" => "Cosmetic".to_string(),
        _ => trimmed.to_string(),
    }
}

/// Extracts a string value from a field, if present and a String.
fn get_string_field<'a>(fields: &'a [(Option<LuaKey>, LuaValue)], key: &str) -> Option<&'a str> {
    fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k {
            if s == key { v.as_str() } else { None }
        } else {
            None
        }
    })
}

/// Extracts a number (f64) from a field, if present and a Number.
fn get_number_field(fields: &[(Option<LuaKey>, LuaValue)], key: &str) -> Option<f64> {
    fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k {
            if s == key { v.as_number() } else { None }
        } else {
            None
        }
    })
}

/// Parses a price from a `LuaValue`, which can be a Number or a Table of currency→amount.
/// If it's a Number and `fallback_currency` is Some, returns Single(currency, number).
/// If it's a Table, returns Multi with all pairs.
fn parse_price(price_val: &LuaValue, fallback_currency: Option<&str>) -> Result<PriceSpec, String> {
    match price_val {
        LuaValue::Number(n) => {
            let cur = fallback_currency
                .ok_or_else(|| "Price is a number but no vendor currency provided".to_string())?;
            Ok(PriceSpec::Single(cur.to_string(), *n))
        }
        LuaValue::Table(pairs) => {
            let mut currencies = Vec::new();
            for (key, val) in pairs {
                let cur = match key {
                    Some(LuaKey::String(s)) => s.clone(),
                    Some(LuaKey::Integer(i)) => i.to_string(),
                    None => return Err("Price table has an unnamed entry".to_string()),
                };
                let amount = val
                    .as_number()
                    .ok_or_else(|| format!("Price table value for '{cur}' is not a number"))?;
                currencies.push((cur, amount));
            }
            if currencies.is_empty() {
                return Err("Price table is empty".to_string());
            }
            Ok(PriceSpec::Multi(currencies))
        }
        LuaValue::String(_) => Err("Price must be a number or a table".to_string()),
    }
}

/// Parses the Prereq field, which can be a number (rank) or a string (quest).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_prereq(table: &[(Option<LuaKey>, LuaValue)]) -> Result<Option<PrereqSpec>, String> {
    for (key, val) in table {
        if let Some(LuaKey::String(s)) = key
            && s == "Prereq"
        {
            match val {
                LuaValue::Number(n) => {
                    let rounded = n.round();
                    if (*n - rounded).abs() < f64::EPSILON && *n >= 0.0 && *n <= f64::from(u32::MAX)
                    {
                        let rank = rounded as u32;
                        return Ok(Some(PrereqSpec::Rank(rank)));
                    }
                    // No else – just return error after the if
                    return Err(format!("Prereq number is not a valid rank: {n}"));
                }
                LuaValue::String(s) => return Ok(Some(PrereqSpec::Quest(s.clone()))),
                LuaValue::Table(_) => return Err("Prereq must be a number or string".to_string()),
            }
        }
    }
    Ok(None)
}

/// Converts a parsed `LuaValue::Table` (representing one offering) into a `RawOffering`.
/// The `vendor_currency` is used as fallback when the price is a single number.
///
/// # Errors
/// Returns a `String` error if the table is malformed (missing required fields).
/// Converts a parsed `LuaValue::Table` (representing one offering) into a `RawOffering`.
/// Returns `Ok(None)` if the table is malformed and should be skipped.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn parse_raw_offering(
    table: &[(Option<LuaKey>, LuaValue)],
    vendor_currency: Option<&str>,
) -> Result<Option<RawOffering>, String> {
    let is_named =
        get_string_field(table, "item").is_some() || get_number_field(table, "cost").is_some();

    if is_named {
        let name = if let Some(n) = get_string_field(table, "item") {
            n.to_string()
        } else {
            tseprintln!("Warning: named offering missing 'item', skipping");
            return Ok(None);
        };
        let category = get_string_field(table, "type")
            .or_else(|| get_string_field(table, "category"))
            .unwrap_or("Misc")
            .to_string();
        let price_val = table
            .iter()
            .find_map(|(k, v)| {
                if let Some(LuaKey::String(s)) = k {
                    if s == "cost" || s == "price" {
                        Some(v)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .ok_or_else(|| "Named offering missing 'cost' or 'price'".to_string())?;
        let price = parse_price(price_val, vendor_currency)?;
        let qty = get_number_field(table, "qty").map_or(1, |n| n as u32);
        let timer = get_number_field(table, "Timer").map(|n| n as u64);
        let limit = get_number_field(table, "Limit").map(|n| n as u32);
        let prereq = parse_prereq(table)?;
        Ok(Some(RawOffering {
            name,
            category: normalize_category(&category),
            price,
            qty,
            prereq,
            timer,
            limit,
        }))
    } else {
        // Positional offering: expects at least name, category, price
        let mut values = Vec::new();
        for (key, val) in table {
            if key.is_none() {
                values.push(val);
            }
        }
        if values.len() < 3 {
            tseprintln!("Warning: positional offering has fewer than 3 fields, skipping");
            tseprintln!("Table fields: {:?}", table); // 👈 print the whole table
            for (key, val) in table {
                tseprintln!("  key={:?}, val={:?}", key, val);
            }
            return Ok(None);
        }
        let name = values[0]
            .as_str()
            .ok_or_else(|| "First positional value must be a string for name".to_string())?
            .to_string();
        let category = values[1]
            .as_str()
            .ok_or_else(|| "Second positional value must be a string for category".to_string())?
            .to_string();
        let price = parse_price(values[2], vendor_currency)?;
        let qty = if values.len() > 3 {
            values[3].as_number().map_or(1, |n| n as u32)
        } else {
            1
        };
        let timer = get_number_field(table, "Timer").map(|n| n as u64);
        let limit = get_number_field(table, "Limit").map(|n| n as u32);
        let prereq = parse_prereq(table)?;
        Ok(Some(RawOffering {
            name,
            category: normalize_category(&category),
            price,
            qty,
            prereq,
            timer,
            limit,
        }))
    }
}

// ========== Vendor Normalizer ==========

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CurrencySpec {
    One(String),
    Many(Vec<String>),
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawVendor {
    pub key: String,
    pub name: String,
    pub currency: CurrencySpec,
    pub vendor_type: Option<String>,
    pub ranks: Option<RankList>,
    pub offerings: Vec<RawOffering>,
}

/// Converts a parsed vendor table (from the Vendors table) into a `RawVendor`.
/// Returns the parsed vendor and the number of offerings that were skipped due to errors.
///
/// # Errors
/// Returns a `String` error if the table is malformed (missing Offerings, etc.).
pub fn parse_raw_vendor(
    vendor_key: String,
    vendor_table: &LuaValue,
) -> Result<(RawVendor, usize), String> {
    let fields = vendor_table
        .as_table()
        .ok_or("Vendor table is not a table")?;

    let name = get_string_field(fields, "Name")
        .unwrap_or(&vendor_key)
        .to_string();

    let currency = parse_currency(fields)?;
    let vendor_type = get_string_field(fields, "Type").map(String::from);

    let ranks = fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k
            && s == "Ranks"
        {
            v.as_table().map(|t| parse_rank_list(t))
        } else {
            None
        }
    });

    let offerings_table = fields
        .iter()
        .find_map(|(k, v)| {
            if let Some(LuaKey::String(s)) = k
                && s == "Offerings"
            {
                v.as_table()
            } else {
                None
            }
        })
        .ok_or("Vendor missing 'Offerings' table")?;

    let vendor_currency_str = match &currency {
        CurrencySpec::One(c) => Some(c.as_str()),
        CurrencySpec::Many(_) | CurrencySpec::None => None,
    };

    let mut offerings = Vec::new();
    let mut skipped = 0usize;
    for (_, offering_val) in offerings_table {
        let fields = if let LuaValue::Table(f) = offering_val {
            f.as_slice()
        } else {
            tseprintln!("Warning: offering entry is not a table, skipping");
            skipped += 1;
            continue;
        };
        match parse_raw_offering(fields, vendor_currency_str) {
            Ok(Some(raw)) => offerings.push(raw),
            Ok(None) => {
                skipped += 1;
            }
            Err(e) => {
                tseprintln!("Warning: failed to parse offering: {}", e);
                skipped += 1;
            }
        }
    }

    Ok((
        RawVendor {
            key: vendor_key,
            name,
            currency,
            vendor_type,
            ranks,
            offerings,
        },
        skipped,
    ))
}

fn parse_currency(fields: &[(Option<LuaKey>, LuaValue)]) -> Result<CurrencySpec, String> {
    for (key, val) in fields {
        if let Some(LuaKey::String(s)) = key
            && s == "Currency"
        {
            return match val {
                LuaValue::String(s) => Ok(CurrencySpec::One(s.clone())),
                LuaValue::Table(pairs) => {
                    let has_keys = pairs.iter().any(|(k, _)| k.is_some());
                    if has_keys {
                        let mut currencies = Vec::new();
                        for (k, _) in pairs {
                            if let Some(LuaKey::String(cur)) = k {
                                currencies.push(cur.clone());
                            } else {
                                return Err("Currency table has non-string key".to_string());
                            }
                        }
                        if currencies.is_empty() {
                            Ok(CurrencySpec::None)
                        } else {
                            Ok(CurrencySpec::Many(currencies))
                        }
                    } else {
                        let mut currencies = Vec::new();
                        for (_, v) in pairs {
                            if let LuaValue::String(s) = v {
                                currencies.push(s.clone());
                            } else {
                                return Err("Currency table value must be a string".to_string());
                            }
                        }
                        if currencies.is_empty() {
                            Ok(CurrencySpec::None)
                        } else {
                            Ok(CurrencySpec::Many(currencies))
                        }
                    }
                }
                LuaValue::Number(_) => {
                    Err("Currency must be a string or a table of strings".to_string())
                }
            };
        }
    }
    Ok(CurrencySpec::None)
}

#[cfg(test)]
mod offering_tests {
    use super::super::lua::test_helpers::*;
    use super::*;

    #[test]
    fn positional_single_currency() {
        let fields = vec![
            (no_key(), string("Kuva")),
            (no_key(), string("Resource")),
            (no_key(), number(10.0)),
            (no_key(), number(5000.0)),
            (key("Timer"), number(604800.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Pathos Clamp"))
            .unwrap()
            .unwrap();
        assert_eq!(result.name, "Kuva");
        assert_eq!(result.category, "Resource");
        match result.price {
            PriceSpec::Single(cur, amt) => {
                assert_eq!(cur, "Pathos Clamp");
                assert_eq!(amt, 10.0);
            }
            _ => panic!("Expected Single"),
        }
        assert_eq!(result.qty, 5000);
        assert_eq!(result.timer, Some(604800));
        assert!(result.prereq.is_none());
        assert!(result.limit.is_none());
    }

    #[test]
    fn positional_missing_qty_defaults_to_1() {
        let fields = vec![
            (no_key(), string("Orokin Reactor")),
            (no_key(), string("Item")),
            (no_key(), number(20.0)),
            (key("Timer"), number(604800.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Platinum"))
            .unwrap()
            .unwrap();
        assert_eq!(result.name, "Orokin Reactor");
        assert_eq!(result.qty, 1);
    }

    #[test]
    fn multi_currency_price() {
        let price_table = table(vec![
            (key("Credits"), number(5000.0)),
            (key("Standing"), number(250.0)),
        ]);
        let fields = vec![
            (no_key(), string("Fosfor Blau (x20) Blueprint")),
            (no_key(), string("Blueprint")),
            (no_key(), price_table),
            (no_key(), number(1.0)),
            (key("Prereq"), number(0.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Platinum"))
            .unwrap()
            .unwrap();
        match result.price {
            PriceSpec::Multi(currencies) => {
                assert_eq!(currencies.len(), 2);
                assert!(currencies.contains(&("Credits".to_string(), 5000.0)));
                assert!(currencies.contains(&("Standing".to_string(), 250.0)));
            }
            _ => panic!("Expected Multi"),
        }
        assert_eq!(result.qty, 1);
        assert_eq!(result.prereq, Some(PrereqSpec::Rank(0)));
    }

    #[test]
    fn string_prereq_quest() {
        let fields = vec![
            (no_key(), string("Exilus Adapter Blueprint")),
            (no_key(), string("Blueprint")),
            (no_key(), number(50000.0)),
            (no_key(), number(1.0)),
            (key("Prereq"), string("Natah (Quest)")),
        ];
        let result = parse_raw_offering(&fields, Some("Standing"))
            .unwrap()
            .unwrap();
        assert_eq!(
            result.prereq,
            Some(PrereqSpec::Quest("Natah (Quest)".to_string()))
        );
    }

    #[test]
    fn numeric_prereq_rank() {
        let fields = vec![
            (no_key(), string("Energy Conversion")),
            (no_key(), string("Mod")),
            (no_key(), number(100000.0)),
            (key("Prereq"), number(5.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Standing"))
            .unwrap()
            .unwrap();
        assert_eq!(result.prereq, Some(PrereqSpec::Rank(5)));
        assert_eq!(result.qty, 1);
    }

    #[test]
    fn timer_and_limit_present() {
        let fields = vec![
            (no_key(), string("Requiem I Relic")),
            (no_key(), string("Relic")),
            (no_key(), number(10.0)),
            (no_key(), number(1.0)),
            (key("Timer"), number(604800.0)),
            (key("Limit"), number(10.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Riven Sliver"))
            .unwrap()
            .unwrap();
        assert_eq!(result.timer, Some(604800));
        assert_eq!(result.limit, Some(10));
    }

    #[test]
    fn named_offering_with_cost() {
        let fields = vec![
            (key("item"), string("Energy Conversion")),
            (key("cost"), number(100000.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Standing"))
            .unwrap()
            .unwrap();
        assert_eq!(result.name, "Energy Conversion");
        assert_eq!(result.category, "Misc");
        match result.price {
            PriceSpec::Single(cur, amt) => {
                assert_eq!(cur, "Standing");
                assert_eq!(amt, 100000.0);
            }
            _ => panic!("Expected Single"),
        }
        assert_eq!(result.qty, 1);
    }

    #[test]
    fn named_offering_with_type_and_qty() {
        let price_table = table(vec![
            (key("Credits"), number(5000.0)),
            (key("Standing"), number(250.0)),
        ]);
        let fields = vec![
            (key("item"), string("Fosfor Blau (x20) Blueprint")),
            (key("type"), string("Blueprint")),
            (key("cost"), price_table),
            (key("qty"), number(1.0)),
        ];
        let result = parse_raw_offering(&fields, None).unwrap().unwrap();
        assert_eq!(result.category, "Blueprint");
        match result.price {
            PriceSpec::Multi(currencies) => {
                assert_eq!(currencies.len(), 2);
            }
            _ => panic!("Expected Multi"),
        }
        assert_eq!(result.qty, 1);
    }

    #[test]
    fn category_normalization() {
        assert_eq!(normalize_category("Resource,"), "Resource");
        assert_eq!(normalize_category("Resource"), "Resource");
        assert_eq!(normalize_category("Cosmetics"), "Cosmetic");
        assert_eq!(normalize_category("Mod"), "Mod");
    }

    #[test]
    fn named_offering_missing_item_returns_ok_none() {
        // Has a 'cost' field (so is_named = true) but no 'item' field — should skip, not error.
        let fields = vec![(key("cost"), number(5000.0)), (key("type"), string("Mod"))];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap();
        assert!(
            result.is_none(),
            "Expected Ok(None) for named offering missing 'item'"
        );
    }

    #[test]
    fn positional_offering_too_few_fields_returns_ok_none() {
        // Only 2 positional values — needs at least 3 (name, category, price).
        let fields = vec![
            (no_key(), string("Orokin Reactor")),
            (no_key(), string("Mod")),
        ];
        let result = parse_raw_offering(&fields, Some("Platinum")).unwrap();
        assert!(
            result.is_none(),
            "Expected Ok(None) for positional offering with < 3 fields"
        );
    }
}

#[cfg(test)]

mod vendor_tests {
    use super::super::lua::test_helpers::*;
    use super::*;

    #[test]
    fn parse_single_currency_vendor() {
        let vendor_table = table(vec![
            (key("Currency"), string("Pathos Clamp")),
            (key("Name"), string("Acrithis")),
            (key("Offerings"), table(vec![])),
        ]);
        let (vendor, skipped) = parse_raw_vendor("Acrithis".to_string(), &vendor_table).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(vendor.key, "Acrithis");
        assert_eq!(vendor.name, "Acrithis");
        assert_eq!(
            vendor.currency,
            CurrencySpec::One("Pathos Clamp".to_string())
        );
        assert!(vendor.vendor_type.is_none());
        assert!(vendor.ranks.is_none());
        assert!(vendor.offerings.is_empty());
    }

    #[test]
    fn parse_multi_currency_vendor() {
        let currency_table = table(vec![
            (no_key(), string("Lyroic Bridge")),
            (no_key(), string("Ren Hypercore")),
            (no_key(), string("Ascaris Prime")),
        ]);
        let vendor_table = table(vec![
            (key("Currency"), currency_table),
            (key("Name"), string("Marie")),
            (key("Offerings"), table(vec![])),
        ]);
        let (vendor, _) = parse_raw_vendor("Marie".to_string(), &vendor_table).unwrap();
        assert_eq!(
            vendor.currency,
            CurrencySpec::Many(vec![
                "Lyroic Bridge".to_string(),
                "Ren Hypercore".to_string(),
                "Ascaris Prime".to_string(),
            ])
        );
    }

    #[test]
    fn parse_no_currency_vendor() {
        let vendor_table = table(vec![
            (key("Name"), string("Star Days")),
            (key("Offerings"), table(vec![])),
        ]);
        let (vendor, _) = parse_raw_vendor("Star Days".to_string(), &vendor_table).unwrap();
        assert_eq!(vendor.currency, CurrencySpec::None);
    }

    #[test]
    fn parse_ranks_with_zero_index() {
        let ranks_table = table(vec![
            (int_key(0), string("Initiation")),
            (no_key(), string("Principled")),
            (no_key(), string("Authentic")),
            (no_key(), string("Lawful")),
            (no_key(), string("Crusader")),
            (no_key(), string("Maxim")),
        ]);
        let vendor_table = table(vec![
            (key("Name"), string("Arbiters of Hexis")),
            (key("Ranks"), ranks_table),
            (key("Offerings"), table(vec![])),
        ]);
        let (vendor, _) = parse_raw_vendor("Arbiters of Hexis".to_string(), &vendor_table).unwrap();
        assert!(vendor.ranks.is_some());
        let ranks = vendor.ranks.unwrap();
        assert!(ranks.zero_indexed);
        assert_eq!(
            ranks.names,
            vec![
                "Initiation",
                "Principled",
                "Authentic",
                "Lawful",
                "Crusader",
                "Maxim"
            ]
        );
    }

    #[test]
    fn parse_ranks_without_zero_index() {
        let ranks_table = table(vec![
            (no_key(), string("Mistral")),
            (no_key(), string("Whirlwind")),
            (no_key(), string("Tempest")),
            (no_key(), string("Hurricane")),
            (no_key(), string("Typhoon")),
        ]);
        let vendor_table = table(vec![
            (key("Name"), string("Conclave")),
            (key("Ranks"), ranks_table),
            (key("Offerings"), table(vec![])),
        ]);
        let (vendor, _) = parse_raw_vendor("Conclave".to_string(), &vendor_table).unwrap();
        assert!(vendor.ranks.is_some());
        let ranks = vendor.ranks.unwrap();
        assert!(!ranks.zero_indexed);
        assert_eq!(
            ranks.names,
            vec!["Mistral", "Whirlwind", "Tempest", "Hurricane", "Typhoon"]
        );
    }
}
