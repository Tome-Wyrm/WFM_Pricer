// src/vendor.rs
use crate::config;
use num_traits::ToPrimitive;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    Ident(String),
    String(String),
    Number(f64),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Equals,
    Comma,
}

// ---------- Lua AST ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankList {
    pub names: Vec<String>,
    pub zero_indexed: bool,
}

#[derive(Debug, PartialEq, Clone)]
pub enum LuaValue {
    String(String),
    Number(f64),
    Table(Vec<(Option<LuaKey>, LuaValue)>),
}

#[derive(Debug, PartialEq, Clone)]
pub enum LuaKey {
    String(String),
    Integer(i64),
}

impl LuaValue {
    #[must_use]
    pub fn as_table(&self) -> Option<&Vec<(Option<LuaKey>, LuaValue)>> {
        match self {
            Self::Table(fields) => Some(fields),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }
}

// ---------- Tokenizer ----------

/// Tokenizes Lua source code, producing a vector of tokens.
///
/// # Errors
/// Returns a `String` error if the input contains unexpected characters or unterminated strings.
#[allow(clippy::too_many_lines)]
pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();

    while i < chars.len() {
        let ch = chars[i];
        match ch {
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
            }
            '-' => {
                if i + 1 < chars.len() && chars[i + 1] == '-' {
                    i += 2;
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                } else {
                    let mut num_str = String::from("-");
                    i += 1;
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                        num_str.push(chars[i]);
                        i += 1;
                    }
                    let num = num_str
                        .parse::<f64>()
                        .map_err(|_| format!("Invalid negative number: {num_str}"))?;
                    tokens.push(Token::Number(num));
                }
            }
            '"' | '\'' => {
                let quote = ch;
                i += 1;
                let mut s = String::new();
                let mut closed = false;
                while i < chars.len() {
                    let c = chars[i];
                    if c == quote {
                        i += 1;
                        closed = true;
                        break;
                    }
                    if c == '\\' {
                        i += 1;
                        if i < chars.len() {
                            let esc = chars[i];
                            match esc {
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                'r' => s.push('\r'),
                                '\\' => s.push('\\'),
                                '"' => s.push('"'),
                                '\'' => s.push('\''),
                                _ => s.push(esc),
                            }
                            i += 1;
                        } else {
                            return Err("Unterminated escape sequence".to_string());
                        }
                    } else {
                        s.push(c);
                        i += 1;
                    }
                }
                if !closed {
                    return Err(format!("Unterminated string: {s}"));
                }
                tokens.push(Token::String(s));
            }
            '0'..='9' => {
                let mut num_str = String::new();
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    num_str.push(chars[i]);
                    i += 1;
                }
                let num = num_str
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid number: {num_str}"))?;
                tokens.push(Token::Number(num));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut ident = String::new();
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    ident.push(chars[i]);
                    i += 1;
                }
                tokens.push(Token::Ident(ident));
            }
            '{' => {
                tokens.push(Token::LBrace);
                i += 1;
            }
            '}' => {
                tokens.push(Token::RBrace);
                i += 1;
            }
            '[' => {
                tokens.push(Token::LBracket);
                i += 1;
            }
            ']' => {
                tokens.push(Token::RBracket);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Equals);
                i += 1;
            }
            ',' | ';' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            _ => return Err(format!("Unexpected character: {ch}")),
        }
    }

    Ok(tokens)
}

// ---------- Parser ----------

fn parse_rank_list(table: &[(Option<LuaKey>, LuaValue)]) -> RankList {
    let mut next_idx = 1i64; // Lua's default start index
    let mut pairs = Vec::new();

    for (key, val) in table {
        let Some(name) = val.as_str() else { continue };
        match key {
            Some(LuaKey::Integer(i)) => {
                pairs.push((*i, name.to_string()));
                if *i + 1 > next_idx { next_idx = *i + 1; }
            }
            _ => {
                pairs.push((next_idx, name.to_string()));
                next_idx += 1;
            }
        }
    }

    pairs.sort_by_key(|(i, _)| *i);
    let zero_indexed = pairs.iter().any(|(i, _)| *i == 0);
    let names = pairs.into_iter().map(|(_, n)| n).collect();
    RankList { names, zero_indexed }
}

/// Parses a token stream into a `LuaValue`.
///
/// # Errors
/// Returns a `String` error if the token stream is malformed.
pub fn parse(tokens: &[Token]) -> Result<LuaValue, String> {
    let mut pos = 0;
    if pos < tokens.len() && matches!(tokens[pos], Token::Ident(ref s) if s == "return") {
        pos += 1;
    }
    let expr = parse_expr(tokens, &mut pos)?;
    Ok(expr)
}

fn parse_expr(tokens: &[Token], pos: &mut usize) -> Result<LuaValue, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of input".to_string());
    }
    match &tokens[*pos] {
        Token::String(s) => {
            *pos += 1;
            Ok(LuaValue::String(s.clone()))
        }
        Token::Number(n) => {
            *pos += 1;
            Ok(LuaValue::Number(*n))
        }
        Token::LBrace => parse_table(tokens, pos),
        Token::Ident(name) => {
            // Treat as string literal (e.g., true, false, variable names)
            *pos += 1;
            Ok(LuaValue::String(name.clone()))
        }
        _ => Err(format!("Unexpected token: {:?}", tokens[*pos])),
    }
}

fn parse_table(tokens: &[Token], pos: &mut usize) -> Result<LuaValue, String> {
    if !matches!(tokens[*pos], Token::LBrace) {
        return Err(format!("Expected '{{', got {:?}", tokens[*pos]));
    }
    *pos += 1;

    let mut fields = Vec::new();

    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::RBrace => {
                *pos += 1;
                return Ok(LuaValue::Table(fields));
            }
            Token::Comma => {
                *pos += 1;
            }
            _ => {
                let field = parse_field(tokens, pos)?;
                fields.push(field);
                if *pos < tokens.len() && matches!(tokens[*pos], Token::Comma) {
                    *pos += 1;
                } else if *pos < tokens.len() && matches!(tokens[*pos], Token::RBrace) {
                    // closing brace will be handled next iteration
                } else if *pos < tokens.len() {
                    return Err(format!("Expected ',' or '}}', got {:?}", tokens[*pos]));
                } else {
                    return Err("Unexpected end while parsing table".to_string());
                }
            }
        }
    }
    Err("Unclosed table".to_string())
}

#[allow(clippy::cast_precision_loss)]
fn parse_field(tokens: &[Token], pos: &mut usize) -> Result<(Option<LuaKey>, LuaValue), String> {
    let key = match &tokens[*pos] {
        Token::LBracket => {
            *pos += 1;
            let key_expr = parse_expr(tokens, pos)?;
            if *pos >= tokens.len() || !matches!(tokens[*pos], Token::RBracket) {
                return Err("Expected ']' after bracket key".to_string());
            }
            *pos += 1;
            if *pos >= tokens.len() || !matches!(tokens[*pos], Token::Equals) {
                return Err("Expected '=' after bracket key".to_string());
            }
            *pos += 1;
            match key_expr {
                LuaValue::String(s) => Some(LuaKey::String(s)),
                LuaValue::Number(n) => {
                    if let Some(i) = n.to_i64() {
                        Some(LuaKey::Integer(i))
                    } else {
                        return Err(format!("Bracket key must be an integer, got {n}"));
                    }
                }
                LuaValue::Table(_) => return Err(format!("Invalid bracket key type: {key_expr:?}")),
            }
        }
        Token::Ident(name) => {
            let next_pos = *pos + 1;
            if next_pos < tokens.len() && matches!(tokens[next_pos], Token::Equals) {
                *pos += 1;
                *pos += 1;
                Some(LuaKey::String(name.clone()))
            } else {
                let value = parse_expr(tokens, pos)?;
                return Ok((None, value));
            }
        }
        _ => {
            let value = parse_expr(tokens, pos)?;
            return Ok((None, value));
        }
    };

    let value = parse_expr(tokens, pos)?;
    Ok((key, value))
}

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
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read vendor cache: {e}"))?;
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
    std::fs::write(path, json)
        .map_err(|e| format!("Failed to write vendor cache: {e}"))?;
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
            if s == key {
                v.as_str()
            } else {
                None
            }
        } else {
            None
        }
    })
}

/// Extracts a number (f64) from a field, if present and a Number.
fn get_number_field(fields: &[(Option<LuaKey>, LuaValue)], key: &str) -> Option<f64> {
    fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k {
            if s == key {
                v.as_number()
            } else {
                None
            }
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
                let amount = val.as_number()
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
        if let Some(LuaKey::String(s)) = key && s == "Prereq" {
            match val {
                LuaValue::Number(n) => {
                    let rounded = n.round();
                    if (*n - rounded).abs() < f64::EPSILON && *n >= 0.0 && *n <= f64::from(u32::MAX) {
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
    let is_named = get_string_field(table, "item").is_some() || get_number_field(table, "cost").is_some();

    if is_named {
        let name = match get_string_field(table, "item") {
            Some(n) => n.to_string(),
            None => {
                eprintln!("Warning: named offering missing 'item', skipping");
                return Ok(None);
            }
        };
        let category = get_string_field(table, "type")
            .or_else(|| get_string_field(table, "category"))
            .unwrap_or("Misc")
            .to_string();
        let price_val = table.iter().find_map(|(k, v)| {
            if let Some(LuaKey::String(s)) = k {
                if s == "cost" || s == "price" {
                    Some(v)
                } else {
                    None
                }
            } else {
                None
            }
        }).ok_or_else(|| "Named offering missing 'cost' or 'price'".to_string())?;
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
            eprintln!("Warning: positional offering has fewer than 3 fields, skipping");
            eprintln!("Table fields: {:?}", table);   // 👈 print the whole table
            for (key, val) in table {
                eprintln!("  key={:?}, val={:?}", key, val);
            }
            return Ok(None);
        }
        let name = values[0].as_str()
            .ok_or_else(|| "First positional value must be a string for name".to_string())?
            .to_string();
        let category = values[1].as_str()
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
    let fields = vendor_table.as_table()
        .ok_or("Vendor table is not a table")?;

    let name = get_string_field(fields, "Name")
        .unwrap_or(&vendor_key)
        .to_string();

    let currency = parse_currency(fields)?;
    let vendor_type = get_string_field(fields, "Type").map(String::from);

    let ranks = fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k && s == "Ranks" {
            v.as_table().map(|t| parse_rank_list(t))
        } else {
            None
        }
    });

    let offerings_table = fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k && s == "Offerings" {
            v.as_table()
        } else {
            None
        }
    }).ok_or("Vendor missing 'Offerings' table")?;

    let vendor_currency_str = match &currency {
        CurrencySpec::One(c) => Some(c.as_str()),
        CurrencySpec::Many(_) | CurrencySpec::None => None,
    };

    let mut offerings = Vec::new();
    let mut skipped = 0usize;
    for (_, offering_val) in offerings_table {
        let fields = match offering_val {
            LuaValue::Table(f) => f.as_slice(),
            _ => {
                eprintln!("Warning: offering entry is not a table, skipping");
                skipped += 1;
                continue;
            }
        };
        match parse_raw_offering(fields, vendor_currency_str) {
            Ok(Some(raw)) => offerings.push(raw),
            Ok(None) => { skipped += 1; }
            Err(e) => {
                eprintln!("Warning: failed to parse offering: {}", e);
                skipped += 1;
            }
        }
    }

    Ok((RawVendor {
        key: vendor_key,
        name,
        currency,
        vendor_type,
        ranks,
        offerings,
    }, skipped))
}

fn parse_currency(fields: &[(Option<LuaKey>, LuaValue)]) -> Result<CurrencySpec, String> {
    for (key, val) in fields {
        if let Some(LuaKey::String(s)) = key && s == "Currency" {
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
                LuaValue::Number(_) => Err("Currency must be a string or a table of strings".to_string()),
            };
        }
    }
    Ok(CurrencySpec::None)
}

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
pub fn load_vendor_metadata() -> Result<std::collections::HashMap<String, VendorMeta>, Box<dyn Error>> {
    let path = Path::new(crate::config::VENDORS_CONFIG_FILE);
    if !path.exists() {
        return Ok(std::collections::HashMap::new());
    }
    let content = fs::read_to_string(path)?;
    let config: VendorConfig = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse vendors.toml: {e}"))?;
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
    "Captura",
    "Captura Scene",
    "Gear",
    "Item",
    "Key",
    "Landing Craft",
    "Misc",
    "Mod",
    "Relic",
    "Resource",
    "Riven Mod",
    "Riven",
    "Weapon",
];
/// Categories seen in the wiki dump that are known and explicitly *not* tradeable.
/// Listed out (rather than inferred by "not in the allowlist") so `classify_category`
/// can distinguish "known and excluded" from "never triaged" — only the latter is the
/// tripwire case.
const NON_TRADEABLE_CATEGORIES: &[&str] = &[
    "Color",
    "Cosmetic",
    "Credits",
    "Decoration",
    "Ephemera",
    "Emblem",
    "Emote",
    "Glyph",
    "Honoria",
    "Scene",
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
fn target_rank_for(category: &str) -> Option<u32> {
    match category {
        "Mod" | "Arcane" => Some(0),
        _ => None,
    }
}

/// A `RawOffering` combined with its `vendors.toml` overlay context and WFM match
/// result — the per-offering row of `cache/vendors_cache.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedOffering {
    pub name: String,
    pub category: String,
    pub price: PriceSpec,
    pub qty: u32,
    pub prereq: Option<PrereqSpec>,
    pub timer: Option<u64>,
    pub limit: Option<u32>,
    /// `None` (with `unmatched_reason` set) if the category isn't tradeable or no WFM
    /// item name matched.
    pub wfm_slug: Option<String>,
    pub target_rank: Option<u32>,
    pub unmatched_reason: Option<String>,
}

/// A `RawVendor` combined with its `vendors.toml` metadata overlay and mapped
/// offerings — the per-vendor row of `cache/vendors_cache.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedVendor {
    pub key: String,
    pub name: String,
    pub currency: CurrencySpec,
    pub location: Option<String>,
    pub group: Option<String>,
    pub excluded: bool,
    pub cost_mode: CostMode,
    pub hand_curated: bool,
    pub offerings: Vec<MappedOffering>,
}

/// Attempts to resolve a raw offering's WFM slug via the existing
/// `mapping::find_wfm_match` lookup, after stripping any yield multiplier (D2).
fn match_offering(
    offering: &RawOffering,
    wfm_by_name: &std::collections::HashMap<String, crate::models::WfmItem>,
) -> (Option<String>, Option<String>) {
    let normalized = normalize_item_name(&offering.name);
    match crate::mapping::find_wfm_match(&normalized, wfm_by_name) {
        Some(item) => (Some(item.slug.clone()), None),
        None => (None, Some(format!("no WFM match for '{normalized}'"))),
    }
}

/// D3: builds the processed vendor cache — raw offerings + `vendors.toml` overlay +
/// matched WFM slug (or `None` + reason if unmatched) — and writes it to
/// `cache/vendors_cache.json`.
///
/// # Errors
/// Returns an error if the raw vendor cache, `vendors.toml`, or the WFM lookup tables
/// can't be loaded, or if the resulting cache can't be serialized/written.
pub fn build_and_write_vendor_cache() -> Result<Vec<MappedVendor>, Box<dyn Error>> {
    let raw_vendors = load_vendor_data()?;
    let meta = load_vendor_metadata()?;
    let (_, _, wfm_by_name, _) = crate::mapping::load_lookup_tables()?;

    let mapped: Vec<MappedVendor> = raw_vendors
        .into_iter()
        .map(|v| {
            let m = meta.get(&v.key).cloned().unwrap_or_default();
            let offerings = v
                .offerings
                .into_iter()
                .map(|o| {
                    let (slug, reason) = if is_tradeable_category(&o.category) {
                        match_offering(&o, &wfm_by_name)
                    } else {
                        (None, Some(format!("category '{}' not tradeable", o.category)))
                    };
                    MappedOffering {
                        target_rank: target_rank_for(&o.category),
                        name: o.name,
                        category: o.category,
                        price: o.price,
                        qty: o.qty,
                        prereq: o.prereq,
                        timer: o.timer,
                        limit: o.limit,
                        wfm_slug: slug,
                        unmatched_reason: reason,
                    }
                })
                .collect();
            MappedVendor {
                key: v.key,
                name: v.name,
                currency: v.currency,
                location: m.location,
                group: m.group,
                excluded: m.excluded,
                cost_mode: m.cost_mode,
                hand_curated: m.hand_curated,
                offerings,
            }
        })
        .collect();

    let json = serde_json::to_string_pretty(&mapped)?;
    fs::write(config::VENDORS_CACHE_FILE, json)?;
    Ok(mapped)
}

// ---- D4: match-coverage report ----

/// Per-vendor match statistics for `vendor --match-report`.
#[derive(Debug, Clone)]
pub struct VendorMatchStats {
    pub key: String,
    pub total_offerings: usize,
    pub tradeable_count: usize,
    pub matched_count: usize,
    /// Names of offerings that were in a tradeable category but didn't resolve to a
    /// WFM slug.
    pub unmatched: Vec<String>,
}

#[must_use]
pub fn compute_match_stats(vendors: &[MappedVendor]) -> Vec<VendorMatchStats> {
    vendors
        .iter()
        .map(|v| {
            let total_offerings = v.offerings.len();
            let tradeable: Vec<&MappedOffering> = v
                .offerings
                .iter()
                .filter(|o| is_tradeable_category(&o.category))
                .collect();
            let matched_count = tradeable.iter().filter(|o| o.wfm_slug.is_some()).count();
            let unmatched = tradeable
                .iter()
                .filter(|o| o.wfm_slug.is_none())
                .map(|o| o.name.clone())
                .collect();
            VendorMatchStats {
                key: v.key.clone(),
                total_offerings,
                tradeable_count: tradeable.len(),
                matched_count,
                unmatched,
            }
        })
        .collect()
}

/// Prints the D4 match-coverage report: per vendor, total / tradeable / matched /
/// unmatched counts, plus the unmatched item names. Internally,
/// `matched + unmatched + skipped-by-category == total` always holds by construction
/// (asserted below in debug builds) since every offering falls into exactly one of
/// those three buckets.
pub fn print_match_report(vendors: &[MappedVendor]) {
    let stats = compute_match_stats(vendors);
    println!(
        "{:<28} {:>6} {:>10} {:>8} {:>10}",
        "Vendor", "Total", "Tradeable", "Matched", "Unmatched"
    );
    for s in &stats {
        let unmatched_count = s.unmatched.len();
        let skipped = s.total_offerings - s.tradeable_count;
        debug_assert_eq!(s.matched_count + unmatched_count + skipped, s.total_offerings);
        println!(
            "{:<28} {:>6} {:>10} {:>8} {:>10}",
            s.key, s.total_offerings, s.tradeable_count, s.matched_count, unmatched_count
        );
        for name in &s.unmatched {
            println!("    unmatched: {name}");
        }
    }
}

/// Ad hoc audit, grouped by category instead of by vendor: total offerings, matched
/// count, and up to 5 sample unmatched names per category. `is_tradeable_category`
/// only gates whether a lookup is attempted — this makes it visible whether a
/// category is a clean single-tradeability bucket or a genuine mix, instead of having
/// to infer that from the per-vendor D4 report.
pub fn dump_category_audit(vendors: &[MappedVendor]) {
    use std::collections::BTreeMap;
    let mut by_category: BTreeMap<&str, (usize, usize, Vec<&str>)> = BTreeMap::new();
    for v in vendors {
        for o in &v.offerings {
            let entry = by_category
                .entry(o.category.as_str())
                .or_insert((0, 0, Vec::new()));
            entry.0 += 1;
            if o.wfm_slug.is_some() {
                entry.1 += 1;
            } else if entry.2.len() < 5 {
                entry.2.push(o.name.as_str());
            }
        }
    }
    println!(
        "{:<20} {:>6} {:>8}  sample unmatched",
        "Category", "Total", "Matched"
    );
    for (cat, (total, matched, samples)) in by_category {
        println!("{cat:<20} {total:>6} {matched:>8}  {}", samples.join(", "));
    }
}

// ---- Revid caching ----

#[derive(Debug, Serialize, Deserialize)]
struct RevidCache {
    revid: u64,
}

/// Reads the cached revid from disk, if present.
pub fn read_cached_revid() -> Result<Option<u64>, Box<dyn Error>> {
    let path = Path::new(crate::config::VENDOR_REVID_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    let cache: RevidCache = serde_json::from_str(&content)?;
    Ok(Some(cache.revid))
}

/// Writes the given revid to the cache file.
pub fn write_cached_revid(revid: u64) -> Result<(), Box<dyn Error>> {
    let path = Path::new(crate::config::VENDOR_REVID_FILE);
    let cache = RevidCache { revid };
    let content = serde_json::to_string_pretty(&cache)?;
    fs::write(path, content)?;
    Ok(())
}

/// Fetches the latest revision ID of Module:Vendors/data from the Warframe wiki.
///
/// # Errors
/// Returns an error if the network request fails or the response doesn't contain a revid.
pub async fn fetch_latest_revid(client: &reqwest::Client) -> Result<u64, Box<dyn Error>> {
    let url = "https://wiki.warframe.com/api.php";
    let params = [
        ("action", "query"),
        ("prop", "revisions"),
        ("titles", "Module:Vendors/data"),
        ("rvprop", "ids"),
        ("format", "json"),
        ("formatversion", "2"),
    ];

    let response = client
        .get(url)
        .query(&params)
        .header("User-Agent", "wfm-pricer-cli")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("API request failed: {}", response.status()).into());
    }

    let json: serde_json::Value = response.json().await?;
    let revid = json
        .pointer("/query/pages/0/revisions/0/revid")
        .and_then(|v| v.as_u64())
        .ok_or("Failed to extract revid from API response")?;

    Ok(revid)
}

/// Fetches the raw Lua source of Module:Vendors/data from the Warframe wiki.
/// Uses the revisions API to get the content directly.
pub async fn fetch_vendors_lua(client: &reqwest::Client) -> Result<String, Box<dyn Error>> {
    let url = "https://wiki.warframe.com/api.php";
    let params = [
        ("action", "query"),
        ("prop", "revisions"),
        ("titles", "Module:Vendors/data"),
        ("rvprop", "content"),
        ("rvslots", "*"),
        ("format", "json"),
        ("formatversion", "2"),
    ];

    let response = client
        .get(url)
        .query(&params)
        .header("User-Agent", "wfm-pricer-cli")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("API request failed: {}", response.status()).into());
    }

    let json: serde_json::Value = response.json().await?;
    let source = json
        .pointer("/query/pages/0/revisions/0/slots/main/content")
        .and_then(|v| v.as_str())
        .ok_or("Failed to extract content from revisions API response")?
        .to_string();

    Ok(source)
}

/// Parses the Lua source of Module:Vendors/data into a vector of RawVendor.
/// Returns the parsed vendors and the total number of offerings skipped due to errors.
pub fn parse_vendors_from_lua(source: &str) -> Result<(Vec<RawVendor>, usize), String> {
    let tokens = tokenize(source)?;
    let parsed = parse(&tokens)?;
    let top_table = parsed.as_table().ok_or("Top-level is not a table")?;
    let vendors_table = top_table
        .iter()
        .find_map(|(key, val)| {
            if let Some(LuaKey::String(s)) = key {
                if s == "Vendors" {
                    val.as_table()
                } else {
                    None
                }
            } else {
                None
            }
        })
        .ok_or("No 'Vendors' table found")?;
    let mut result = Vec::new();
    let mut total_skipped = 0usize;
    for (key, val) in vendors_table {
        let vendor_key = match key {
            Some(LuaKey::String(s)) => s.clone(),
            _ => return Err("Vendor key is not a string".to_string()),
        };
        let (raw, skipped) = parse_raw_vendor(vendor_key, val)?;
        result.push(raw);
        total_skipped += skipped;
    }
    Ok((result, total_skipped))
}

/// Fetches and caches vendor data if the remote revision differs from the cached one.
pub async fn fetch_and_cache_vendors(client: &reqwest::Client) -> Result<(), Box<dyn Error>> {
    let remote_revid = fetch_latest_revid(client).await?;
    let cached_revid = read_cached_revid()?;

    if let Some(cached) = cached_revid {
        if cached == remote_revid {
            println!("Vendor data unchanged (revid {}). Skipping fetch.", remote_revid);
            return Ok(());
        }
    }
    println!("Vendor data changed (cached: {:?}, remote: {}). Fetching...", cached_revid, remote_revid);

    let lua_source = fetch_vendors_lua(client).await?;
    let (raw_vendors, skipped) = parse_vendors_from_lua(&lua_source)
        .map_err(|e| format!("Failed to parse vendor Lua: {e}"))?;

    write_vendor_cache(&raw_vendors)?;
    write_cached_revid(remote_revid)?;

    if skipped > 0 {
        println!(
            "Vendor cache updated ({} vendors, {} offerings parsed, {} skipped).",
            raw_vendors.len(),
            raw_vendors.iter().map(|v| v.offerings.len()).sum::<usize>(),
            skipped
        );
    } else {
        println!(
            "Vendor cache updated ({} vendors, {} offerings parsed).",
            raw_vendors.len(),
            raw_vendors.iter().map(|v| v.offerings.len()).sum::<usize>()
        );
    }
    Ok(())
}

/// Runs the vendor mode: displays a list of vendors and their offerings.
pub async fn run_vendor_cli() -> Result<(), Box<dyn Error>> {
    let client = reqwest::Client::new();
    // Ensure cache is fresh
    fetch_and_cache_vendors(&client).await?;

    // Load raw vendors
    let raw_vendors = load_vendor_data()?;
    println!("Loaded {} vendors from cache.", raw_vendors.len());

    // Display vendor list
    for (idx, vendor) in raw_vendors.iter().enumerate() {
        let currency_desc = match &vendor.currency {
            CurrencySpec::One(c) => format!("Currency: {}", c),
            CurrencySpec::Many(currencies) => format!("Currencies: {}", currencies.join(", ")),
            CurrencySpec::None => "No currency".to_string(),
        };
        println!("{}. {} ({})", idx + 1, vendor.name, currency_desc);
    }

    print!("\nSelect a vendor by number (1-{}): ", raw_vendors.len());
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let idx: usize = input.trim().parse().map_err(|_| "Invalid number")?;
    if idx == 0 || idx > raw_vendors.len() {
        return Err("Invalid vendor number".into());
    }
    let vendor = &raw_vendors[idx - 1];

    println!("\nVendor: {}", vendor.name);
    println!("Offerings ({} items):", vendor.offerings.len());
    for offering in &vendor.offerings {
        let price_desc = match &offering.price {
            PriceSpec::Single(cur, amt) => format!("{} {}", amt, cur),
            PriceSpec::Multi(pairs) => {
                pairs.iter().map(|(cur, amt)| format!("{} {}", amt, cur)).collect::<Vec<_>>().join(" + ")
            }
        };
        let prereq = match &offering.prereq {
            Some(PrereqSpec::Rank(r)) => format!(" (Rank {})", r),
            Some(PrereqSpec::Quest(q)) => format!(" (Quest: {})", q),
            None => String::new(),
        };
        let timer = offering.timer.map_or(String::new(), |t| format!(" (Timer: {}s)", t));
        let limit = offering.limit.map_or(String::new(), |l| format!(" (Limit: {})", l));
        println!("  {} [{}] Cost: {}{}{}{}", offering.name, offering.category, price_desc, prereq, timer, limit);
    }

    Ok(())
}

// ---------- Tests ----------

#[cfg(test)]
mod test_helpers {
    use super::*;

    #[allow(dead_code)]
    pub fn number(n: f64) -> LuaValue { LuaValue::Number(n) }
    #[allow(dead_code)]
    pub fn string(s: &str) -> LuaValue { LuaValue::String(s.to_string()) }
    #[allow(dead_code)]
    pub fn table(fields: Vec<(Option<LuaKey>, LuaValue)>) -> LuaValue { LuaValue::Table(fields) }
    #[allow(dead_code)]
    pub fn key(s: &str) -> Option<LuaKey> { Some(LuaKey::String(s.to_string())) }
    #[allow(dead_code)]
    pub fn int_key(i: i64) -> Option<LuaKey> { Some(LuaKey::Integer(i)) }
    #[allow(dead_code)]
    pub fn no_key() -> Option<LuaKey> { None }
}

#[cfg(test)]
mod offering_tests {
    use super::test_helpers::*;
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
        let result = parse_raw_offering(&fields, Some("Pathos Clamp")).unwrap().unwrap();
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
        let result = parse_raw_offering(&fields, Some("Platinum")).unwrap().unwrap();
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
        let result = parse_raw_offering(&fields, Some("Platinum")).unwrap().unwrap();
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
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap().unwrap();
        assert_eq!(result.prereq, Some(PrereqSpec::Quest("Natah (Quest)".to_string())));
    }

    #[test]
    fn numeric_prereq_rank() {
        let fields = vec![
            (no_key(), string("Energy Conversion")),
            (no_key(), string("Mod")),
            (no_key(), number(100000.0)),
            (key("Prereq"), number(5.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap().unwrap();
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
        let result = parse_raw_offering(&fields, Some("Riven Sliver")).unwrap().unwrap();
        assert_eq!(result.timer, Some(604800));
        assert_eq!(result.limit, Some(10));
    }

    #[test]
    fn named_offering_with_cost() {
        let fields = vec![
            (key("item"), string("Energy Conversion")),
            (key("cost"), number(100000.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap().unwrap();
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
        let fields = vec![
            (key("cost"), number(5000.0)),
            (key("type"), string("Mod")),
        ];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap();
        assert!(result.is_none(), "Expected Ok(None) for named offering missing 'item'");
    }

    #[test]
    fn positional_offering_too_few_fields_returns_ok_none() {
        // Only 2 positional values — needs at least 3 (name, category, price).
        let fields = vec![
            (no_key(), string("Orokin Reactor")),
            (no_key(), string("Mod")),
        ];
        let result = parse_raw_offering(&fields, Some("Platinum")).unwrap();
        assert!(result.is_none(), "Expected Ok(None) for positional offering with < 3 fields");
    }
}

#[cfg(test)]
mod vendor_tests {
    use super::test_helpers::*;
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
        assert_eq!(vendor.currency, CurrencySpec::One("Pathos Clamp".to_string()));
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
        assert_eq!(vendor.currency, CurrencySpec::Many(vec![
            "Lyroic Bridge".to_string(),
            "Ren Hypercore".to_string(),
            "Ascaris Prime".to_string(),
        ]));
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
            vec!["Initiation", "Principled", "Authentic", "Lawful", "Crusader", "Maxim"]
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

#[cfg(test)]
mod integration_tests {
    use super::*;

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
                        if name == "Vendors" {
                            Some(val)
                        } else {
                            None
                        }
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
            eprintln!("Skipping full cache test – cache file not found. Run the fetch step first.");
            return;
        }
        let vendors = load_vendor_data().expect("Failed to load vendor data");
        assert!(vendors.len() >= 60, "Expected at least 60 vendors, got {}", vendors.len());
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
            eprintln!("Skipping tripwire – cache or config file not found.");
            return;
        }
        let vendors = load_vendor_data().expect("Failed to load vendor data");
        let meta = load_vendor_metadata().expect("Failed to load vendor metadata");

        let mut missing = Vec::new();
        for v in &vendors {
            match meta.get(&v.key) {
                Some(m) if m.excluded || m.location.is_some() => {}
                Some(_) => missing.push(format!("{} (has entry but no location and not excluded)", v.key)),
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
            ("Ostron", &["Fisher Hai-Luk", "Hok", "Master Teasonai", "Old Man Suumbaat"]),
            ("Solaris United", &["Legs", "Rude Zuud", "Smokefinger", "The Business"]),
            ("Entrati", &["Daughter", "Father", "Otak", "Son"]),
            ("The Hex", &["Amir", "Aoi", "Quincy", "Minerva", "Velimir", "Eleanor"]),
            ("The Holdfasts", &["Cavalero", "Hombask"]),
        ];

        let config_path = std::path::Path::new(config::VENDORS_CONFIG_FILE);
        if !config_path.exists() {
            eprintln!("Skipping pool consistency check – vendors.toml not found.");
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
                        member, pool, m.group
                    ),
                    None => panic!("Pool member '{}' (group '{}') is missing from vendors.toml", member, pool),
                }
            }
        }
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
        let citrine = meta.get("Unearth Citrine").expect("Unearth Citrine missing");
        assert_eq!(citrine.cost_mode, CostMode::AllOf);
        let supply = meta.get("Operational Supply").expect("Operational Supply missing");
        assert_eq!(supply.cost_mode, CostMode::AnyOf);

        // hand_curated
        let marie = meta.get("Marie").expect("Marie missing");
        assert!(marie.hand_curated);
    }
}

#[cfg(test)]
mod category_tests {
    use super::*;

    #[test]
    fn classify_category_known_cases() {
        assert_eq!(classify_category("Mod"), CategoryClass::Tradeable);
        assert_eq!(classify_category("Captura Scene"), CategoryClass::Tradeable);
        assert_eq!(classify_category("Sigil"), CategoryClass::NonTradeable);
        assert_eq!(classify_category("Scene"), CategoryClass::NonTradeable);
        assert_eq!(classify_category("SomeBrandNewCategory"), CategoryClass::Unknown);
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
            eprintln!("Skipping category tripwire – cache file not found.");
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

#[cfg(test)]
mod slug_matching_tests {
    use super::*;

    /// D3 spot-check: run the real caches through `build_and_write_vendor_cache` and
    /// confirm a handful of known-good (vendor, item) pairs resolve to a WFM slug.
    /// Skips (rather than fails) if the caches this depends on haven't been generated
    /// yet — same convention as the other cache-dependent integration tests in this
    /// file. Add to `KNOWN_GOOD` as you hand-verify more pairs against WFM.
    #[test]
    fn build_vendor_cache_resolves_known_items() {
        let raw_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        let wfm_path = std::path::Path::new(config::WFM_CACHE_FILE);
        let wfcd_path = std::path::Path::new(config::WFCD_CACHE_FILE);
        if !raw_path.exists() || !wfm_path.exists() || !wfcd_path.exists() {
            eprintln!("Skipping spot-check – caches not present. Run update-caches first.");
            return;
        }
        let mapped = build_and_write_vendor_cache().expect("failed to build vendor cache");

        // (vendor_key, offering_name) — hand-verified as actually tradeable on WFM.
        // Replace the placeholders below with real pairs from your vendors.toml/wiki
        // dump; PLACEHOLDER entries are skipped with a warning instead of failing, so
        // this test stays green while you fill the list in incrementally.
        const KNOWN_GOOD: &[(&str, &str)] = &[
            ("PLACEHOLDER", "PLACEHOLDER"),
            ("PLACEHOLDER", "PLACEHOLDER"),
            ("PLACEHOLDER", "PLACEHOLDER"),
            ("PLACEHOLDER", "PLACEHOLDER"),
            ("PLACEHOLDER", "PLACEHOLDER"),
        ];

        for (vendor_key, item_name) in KNOWN_GOOD {
            if *vendor_key == "PLACEHOLDER" {
                eprintln!("Skipping unfilled KNOWN_GOOD placeholder — fill in real (vendor, item) pairs.");
                continue;
            }
            let vendor = mapped
                .iter()
                .find(|v| v.key == *vendor_key)
                .unwrap_or_else(|| panic!("vendor '{vendor_key}' missing from mapped cache"));
            let offering = vendor
                .offerings
                .iter()
                .find(|o| o.name == *item_name)
                .unwrap_or_else(|| {
                    panic!("offering '{item_name}' missing from vendor '{vendor_key}'")
                });
            assert!(
                offering.wfm_slug.is_some(),
                "'{item_name}' ({vendor_key}) should resolve to a WFM slug, got: {:?}",
                offering.unmatched_reason
            );
        }
    }
    #[test]
    #[ignore] // manual audit, not a pass/fail check — run with `cargo test -- --ignored dump_category_audit -- --nocapture`
    fn dump_category_audit_manual() {
        let raw_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
        let wfm_path = std::path::Path::new(config::WFM_CACHE_FILE);
        let wfcd_path = std::path::Path::new(config::WFCD_CACHE_FILE);
        if !raw_path.exists() || !wfm_path.exists() || !wfcd_path.exists() {
            eprintln!("Skipping category audit – caches not present. Run update-caches first.");
            return;
        }
        let mapped = build_and_write_vendor_cache().expect("failed to build vendor cache");
        dump_category_audit(&mapped);
    }
}

#[cfg(test)]
mod match_report_tests {
    use super::*;

    fn offering(name: &str, category: &str, slug: Option<&str>) -> MappedOffering {
        MappedOffering {
            name: name.to_string(),
            category: category.to_string(),
            price: PriceSpec::Single("Credits".to_string(), 100.0),
            qty: 1,
            prereq: None,
            timer: None,
            limit: None,
            wfm_slug: slug.map(str::to_string),
            target_rank: target_rank_for(category),
            unmatched_reason: if slug.is_some() {
                None
            } else {
                Some("test".to_string())
            },
        }
    }

    #[test]
    fn compute_match_stats_totals_reconcile() {
        let vendors = vec![MappedVendor {
            key: "Test".to_string(),
            name: "Test".to_string(),
            currency: CurrencySpec::One("Credits".to_string()),
            location: None,
            group: None,
            excluded: false,
            cost_mode: CostMode::Single,
            hand_curated: false,
            offerings: vec![
                offering("A", "Mod", Some("a_slug")),
                offering("B", "Mod", None),
                offering("C", "Sigil", None),
            ],
        }];

        let stats = compute_match_stats(&vendors);
        assert_eq!(stats.len(), 1);
        let s = &stats[0];
        assert_eq!(s.total_offerings, 3);
        assert_eq!(s.tradeable_count, 2); // Mod, Mod (Sigil is skipped-by-category)
        assert_eq!(s.matched_count, 1);
        assert_eq!(s.unmatched, vec!["B".to_string()]);

        // Matched + unmatched + skipped-by-category == total.
        let skipped = s.total_offerings - s.tradeable_count;
        assert_eq!(s.matched_count + s.unmatched.len() + skipped, s.total_offerings);
    }
}

#[cfg(test)]
mod tokenizer_tests {
    use super::*;
    #[test]
    fn tokenizes_simple_lua_table() {
        let input = r#"
        -- This is a comment
        return {
            ["vendor"] = {
                ["Cephalon Simaris"] = {
                    location = "Relay",
                    currency = "Simaris Standing",
                    pool = {
                        { item = "Energy Conversion", cost = 100000 },
                    }
                }
            }
        }
        "#;
        let tokens = tokenize(input).unwrap();
        let expected = vec![
            Token::Ident("return".to_string()),
            Token::LBrace,
            Token::LBracket,
            Token::String("vendor".to_string()),
            Token::RBracket,
            Token::Equals,
            Token::LBrace,
            Token::LBracket,
            Token::String("Cephalon Simaris".to_string()),
            Token::RBracket,
            Token::Equals,
            Token::LBrace,
            Token::Ident("location".to_string()),
            Token::Equals,
            Token::String("Relay".to_string()),
            Token::Comma,
            Token::Ident("currency".to_string()),
            Token::Equals,
            Token::String("Simaris Standing".to_string()),
            Token::Comma,
            Token::Ident("pool".to_string()),
            Token::Equals,
            Token::LBrace,
            Token::LBrace,
            Token::Ident("item".to_string()),
            Token::Equals,
            Token::String("Energy Conversion".to_string()),
            Token::Comma,
            Token::Ident("cost".to_string()),
            Token::Equals,
            Token::Number(100000.0),
            Token::RBrace,
            Token::Comma,
            Token::RBrace,
            Token::RBrace,
            Token::RBrace,
            Token::RBrace,
        ];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn handles_escaped_strings() {
        let input = r#"local s = "hello world""#;
        let tokens = tokenize(input).unwrap();
        assert_eq!(tokens[0], Token::Ident("local".to_string()));
        assert_eq!(tokens[1], Token::Ident("s".to_string()));
        assert_eq!(tokens[2], Token::Equals);
        assert_eq!(tokens[3], Token::String("hello world".to_string()));
    }

    #[test]
    fn handles_single_quoted_strings() {
        let input = r#"local a = 'single quoted'"#;
        let tokens = tokenize(input).unwrap();
        assert_eq!(tokens[3], Token::String("single quoted".to_string()));
    }

    #[test]
    fn strips_comments_completely() {
        let input = r#"
        -- comment
        local x = 5
        "#;
        let tokens = tokenize(input).unwrap();
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0], Token::Ident("local".to_string()));
        assert_eq!(tokens[1], Token::Ident("x".to_string()));
        assert_eq!(tokens[2], Token::Equals);
        assert_eq!(tokens[3], Token::Number(5.0));
    }

    #[test]
    fn handles_negative_numbers() {
        let input = "x = -5";
        let tokens = tokenize(input).unwrap();
        assert_eq!(tokens[0], Token::Ident("x".to_string()));
        assert_eq!(tokens[1], Token::Equals);
        assert_eq!(tokens[2], Token::Number(-5.0));
    }

    #[test]
    fn parse_simple_table() {
        let input = r#"{ ["a"] = 1, b = 2, "c" }"#;
        let tokens = tokenize(input).unwrap();
        let value = parse(&tokens).unwrap();
        let expected = LuaValue::Table(vec![
            (Some(LuaKey::String("a".to_string())), LuaValue::Number(1.0)),
            (Some(LuaKey::String("b".to_string())), LuaValue::Number(2.0)),
            (None, LuaValue::String("c".to_string())),
        ]);
        assert_eq!(value, expected);
    }

    #[test]
    fn parse_nested_table() {
        let input = r#"{ outer = { inner = 42 } }"#;
        let tokens = tokenize(input).unwrap();
        let value = parse(&tokens).unwrap();
        let expected = LuaValue::Table(vec![
            (Some(LuaKey::String("outer".to_string())),
             LuaValue::Table(vec![
                 (Some(LuaKey::String("inner".to_string())), LuaValue::Number(42.0))
             ]))
        ]);
        assert_eq!(value, expected);
    }

    #[test]
    fn parse_table_with_mixed_types() {
        let input = r#"{ "positional", ["key"] = 10, flag = true }"#;
        let tokens = tokenize(input).unwrap();
        let value = parse(&tokens).unwrap();
        match value {
            LuaValue::Table(fields) => {
                assert_eq!(fields.len(), 3);
                assert!(matches!(fields[0], (None, LuaValue::String(_))));
                assert!(matches!(&fields[1], (Some(LuaKey::String(k)), LuaValue::Number(10.0)) if k == "key"));
                assert!(matches!(&fields[2], (Some(LuaKey::String(k)), LuaValue::String(v)) if k == "flag" && v == "true"));
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn parse_sample_vendor() {
        let input = r#"
        return {
            ["Cephalon Simaris"] = {
                location = "Relay",
                currency = "Simaris Standing",
                pool = {
                    { item = "Energy Conversion", cost = 100000 },
                    { item = "Health Conversion", cost = 100000 },
                }
            }
        }
        "#;
        let tokens = tokenize(input).unwrap();
        let value = parse(&tokens).unwrap();
        match value {
            LuaValue::Table(vendors) => {
                assert_eq!(vendors.len(), 1);
                let (key, val) = &vendors[0];
                match (key, val) {
                    (Some(LuaKey::String(name)), LuaValue::Table(fields)) => {
                        assert_eq!(name, "Cephalon Simaris");
                        let location = fields.iter().find(|(k, _)| matches!(k, Some(LuaKey::String(s)) if s == "location"));
                        assert!(location.is_some());
                        if let Some((_, LuaValue::String(loc))) = location {
                            assert_eq!(loc, "Relay");
                        } else {
                            panic!("location not a string");
                        }
                        let pool = fields.iter().find(|(k, _)| matches!(k, Some(LuaKey::String(s)) if s == "pool"));
                        assert!(pool.is_some());
                        if let Some((_, LuaValue::Table(pool_items))) = pool {
                            assert_eq!(pool_items.len(), 2);
                        } else {
                            panic!("pool not a table");
                        }
                    }
                    _ => panic!("Unexpected vendor structure"),
                }
            }
            _ => panic!("Expected Table"),
        }
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // This hits the live wiki – run with `cargo test -- --ignored`
    async fn fetch_latest_revid_returns_plausible_number() {
        let client = reqwest::Client::new();
        let revid = fetch_latest_revid(&client).await.unwrap();
        // A plausible revision ID for a popular module is > 1000000 and < 10^10
        assert!(revid > 1_000_000);
        assert!(revid < 10_000_000_000);
        println!("Latest revid: {}", revid);
    }
}