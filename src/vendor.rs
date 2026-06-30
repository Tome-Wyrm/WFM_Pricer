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

/// Extracts the Lua source string from the `MediaWiki` API response JSON.
#[allow(dead_code)]
fn extract_lua_content(json: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("Invalid JSON: {e}"))?;

    let content = v
        .pointer("/query/pages")
        .and_then(|pages| pages.as_object())
        .and_then(|pages| pages.values().next())
        .and_then(|page| page.get("revisions"))
        .and_then(|revs| revs.as_array())
        .and_then(|arr| arr.first())
        .and_then(|rev| rev.get("slots"))
        .and_then(|slots| slots.get("main"))
        .and_then(|main| main.get("*"))
        .and_then(|c| c.as_str())
        .ok_or("Could not find Lua source in the cache JSON")?;

    Ok(content.to_string())
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
            // Not enough fields – skip this offering
            eprintln!("Warning: positional offering has fewer than 3 fields, skipping");
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
///
/// # Errors
/// Returns a `String` error if the table is malformed (missing Offerings, etc.).
pub fn parse_raw_vendor(
    vendor_key: String,
    vendor_table: &LuaValue,
) -> Result<RawVendor, String> {
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
    for offering_table in offerings_table {
        match parse_raw_offering(std::slice::from_ref(offering_table), vendor_currency_str) {
            Ok(Some(raw)) => offerings.push(raw),
            Ok(None) => { /* skipped */ }
            Err(e) => eprintln!("Warning: failed to parse offering: {}", e),
        }
    }

    Ok(RawVendor {
        key: vendor_key,
        name,
        currency,
        vendor_type,
        ranks,
        offerings,
    })
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
    let url = "https://warframe.fandom.com/api.php";
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
    let url = "https://warframe.fandom.com/api.php";
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
pub fn parse_vendors_from_lua(source: &str) -> Result<Vec<RawVendor>, String> {
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
    for (key, val) in vendors_table {
        let vendor_key = match key {
            Some(LuaKey::String(s)) => s.clone(),
            _ => return Err("Vendor key is not a string".to_string()),
        };
        let raw = parse_raw_vendor(vendor_key, val)?;
        result.push(raw);
    }
    Ok(result)
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
    let raw_vendors = parse_vendors_from_lua(&lua_source)
        .map_err(|e| format!("Failed to parse vendor Lua: {e}"))?;

    write_vendor_cache(&raw_vendors)?;
    write_cached_revid(remote_revid)?;

    println!("Vendor cache updated successfully ({} vendors).", raw_vendors.len());
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
        let vendor = parse_raw_vendor("Acrithis".to_string(), &vendor_table).unwrap();
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
        let vendor = parse_raw_vendor("Marie".to_string(), &vendor_table).unwrap();
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
        let vendor = parse_raw_vendor("Star Days".to_string(), &vendor_table).unwrap();
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
        let vendor = parse_raw_vendor("Arbiters of Hexis".to_string(), &vendor_table).unwrap();
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
        let vendor = parse_raw_vendor("Conclave".to_string(), &vendor_table).unwrap();
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
        let fixture_path = "tests/fixtures/vendors_sample.json";
        let json_str = std::fs::read_to_string(fixture_path)
            .expect("Fixture file missing");
        let lua_source = extract_lua_content(&json_str).expect("Failed to extract Lua");
        let tokens = tokenize(&lua_source).expect("Tokenizer failed");
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
                assert!(!offerings.is_empty());
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
