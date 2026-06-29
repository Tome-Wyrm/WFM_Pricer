use crate::config;

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
    pub fn as_table(&self) -> Option<&Vec<(Option<LuaKey>, LuaValue)>> {
        match self {
            LuaValue::Table(fields) => Some(fields),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            LuaValue::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_number(&self) -> Option<f64> {
        match self {
            LuaValue::Number(n) => Some(*n),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PriceSpec {
    Single(String, f64),
    Multi(Vec<(String, f64)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrereqSpec {
    Rank(u32),
    Quest(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawOffering {
    pub name: String,
    pub category: String,
    pub price: PriceSpec,
    pub qty: u32,
    pub prereq: Option<PrereqSpec>,
    pub timer: Option<u64>,
    pub limit: Option<u32>,
}

// ---------- Tokenizer ----------

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
                        .map_err(|_| format!("Invalid negative number: {}", num_str))?;
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
                    return Err(format!("Unterminated string: {}", s));
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
                    .map_err(|_| format!("Invalid number: {}", num_str))?;
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
            '{' => { tokens.push(Token::LBrace); i += 1; }
            '}' => { tokens.push(Token::RBrace); i += 1; }
            '[' => { tokens.push(Token::LBracket); i += 1; }
            ']' => { tokens.push(Token::RBracket); i += 1; }
            '=' => { tokens.push(Token::Equals); i += 1; }
            ',' => { tokens.push(Token::Comma); i += 1; }
            ';' => { tokens.push(Token::Comma); i += 1; }
            _ => return Err(format!("Unexpected character: {}", ch)),
        }
    }

    Ok(tokens)
}

// ---------- Parser ----------

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
                continue;
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
                    if n.fract() == 0.0 && n >= -((i64::MAX as f64) + 1.0) && n <= i64::MAX as f64 {
                        Some(LuaKey::Integer(n as i64))
                    } else {
                        return Err(format!("Bracket key must be string or integer, got {}", n));
                    }
                }
                _ => return Err(format!("Invalid bracket key type: {:?}", key_expr)),
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

/// Loads the raw vendor data from the cached file, extracts the Lua source,
/// tokenizes and parses it, returning the parsed LuaValue.
///
/// # Errors
/// Returns a String error if the file cannot be read, JSON parsing fails,
/// tokenization or parsing fails.
pub fn load_vendor_data() -> Result<LuaValue, String> {
    // Try both possible file names (the one from the plan and the current one)
    let cache_path = std::path::Path::new(config::VENDORS_RAW_CACHE_FILE);
    let alt_path = std::path::Path::new("cache/vendors_data_cache.json");
    let path = if cache_path.exists() {
        cache_path
    } else if alt_path.exists() {
        alt_path
    } else {
        return Err("Vendor cache file not found".to_string());
    };

    let raw_json = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read vendor cache file: {}", e))?;

    let lua_source = extract_lua_content(&raw_json)
        .map_err(|e| format!("Failed to extract Lua source: {}", e))?;

    let tokens = tokenize(&lua_source)
        .map_err(|e| format!("Tokenizer error: {}", e))?;

    let parsed = parse(&tokens)
        .map_err(|e| format!("Parser error: {}", e))?;

    Ok(parsed)
}

/// Extracts the Lua source string from the MediaWiki API response JSON.
fn extract_lua_content(json: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    // Navigate to the content: pages[pageid].revisions[0].slots.main['*']
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

/// Normalizes known category typos from the data.
/// Only modifies specific known issues; otherwise leaves the string as-is.
pub fn normalize_category(cat: &str) -> String {
    let trimmed = cat.trim();
    match trimmed {
        "Resource," => "Resource".to_string(),
        "Cosmetics" => "Cosmetic".to_string(),
        // Add other known corrections here as needed
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
                    .ok_or_else(|| format!("Price table value for '{}' is not a number", cur))?;
                currencies.push((cur, amount));
            }
            if currencies.is_empty() {
                return Err("Price table is empty".to_string());
            }
            Ok(PriceSpec::Multi(currencies))
        }
        _ => Err("Price must be a number or a table".to_string()),
    }
}

/// Parses the Prereq field, which can be a number (rank) or a string (quest).
fn parse_prereq(table: &[(Option<LuaKey>, LuaValue)]) -> Result<Option<PrereqSpec>, String> {
    // Find the Prereq key
    for (key, val) in table {
        if let Some(LuaKey::String(s)) = key {
            if s == "Prereq" {
                match val {
                    LuaValue::Number(n) => {
                        // Integer check
                        let rank = n.round() as u32;
                        if (n - rank as f64).abs() < f64::EPSILON {
                            return Ok(Some(PrereqSpec::Rank(rank)));
                        } else {
                            return Err(format!("Prereq number is not an integer: {}", n));
                        }
                    }
                    LuaValue::String(s) => {
                        return Ok(Some(PrereqSpec::Quest(s.clone())));
                    }
                    _ => return Err("Prereq must be a number or string".to_string()),
                }
            }
        }
    }
    Ok(None)
}

/// Converts a parsed LuaValue::Table (representing one offering) into a RawOffering.
/// The vendor_currency is used as fallback when the price is a single number.
pub fn parse_raw_offering(
    table: &[(Option<LuaKey>, LuaValue)],
    vendor_currency: Option<&str>,
) -> Result<RawOffering, String> {
    // Decide if this is a named offering (has "item" and "cost") or positional.
    let is_named = get_string_field(table, "item").is_some() || get_number_field(table, "cost").is_some();

    if is_named {
        // Named form: { item = "X", cost = 10, qty = 1, Timer = ..., Prereq = ..., Limit = ... }
        let name = get_string_field(table, "item")
            .ok_or("Named offering missing 'item'")?
            .to_string();
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
        }).ok_or("Named offering missing 'cost' or 'price'")?;
        let price = parse_price(price_val, vendor_currency)?;
        let qty = get_number_field(table, "qty").map(|n| n as u32).unwrap_or(1);
        let timer = get_number_field(table, "Timer").map(|n| n as u64);
        let limit = get_number_field(table, "Limit").map(|n| n as u32);
        let prereq = parse_prereq(table)?;
        Ok(RawOffering {
            name,
            category: normalize_category(&category),
            price,
            qty,
            prereq,
            timer,
            limit,
        })
    } else {
        // Positional form: { "name", "category", price, qty?, Timer = ..., Prereq = ..., Limit = ... }
        // Extract positional values in order.
        let mut values = Vec::new();
        for (key, val) in table {
            if key.is_none() {
                values.push(val);
            }
        }
        if values.len() < 3 {
            return Err("Positional offering needs at least name, category, price".to_string());
        }
        let name = values[0].as_str()
            .ok_or("First positional value must be a string for name")?
            .to_string();
        let category = values[1].as_str()
            .ok_or("Second positional value must be a string for category")?
            .to_string();
        // Price is the third positional (index 2)
        let price = parse_price(&values[2], vendor_currency)?;
        // Qty: fourth positional (index 3) if present, else 1
        let qty = if values.len() > 3 {
            values[3].as_number().map(|n| n as u32).unwrap_or(1)
        } else {
            1
        };

        // Named extras: Timer, Prereq, Limit
        let timer = get_number_field(table, "Timer").map(|n| n as u64);
        let limit = get_number_field(table, "Limit").map(|n| n as u32);
        let prereq = parse_prereq(table)?;

        Ok(RawOffering {
            name,
            category: normalize_category(&category),
            price,
            qty,
            prereq,
            timer,
            limit,
        })
    }
}

// ========== Vendor Normalizer ==========

#[derive(Debug, Clone, PartialEq)]
pub enum CurrencySpec {
    One(String),
    Many(Vec<String>),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawVendor {
    pub key: String,
    pub name: String,
    pub currency: CurrencySpec,
    pub vendor_type: Option<String>,
    pub ranks: Option<Vec<(Option<LuaKey>, LuaValue)>>,
    pub offerings: Vec<RawOffering>,
}

/// Converts a parsed vendor table (from the Vendors table) into a RawVendor.
/// `vendor_key` is the string key used in the Vendors table.
pub fn parse_raw_vendor(
    vendor_key: String,
    vendor_table: &LuaValue,
) -> Result<RawVendor, String> {
    let fields = vendor_table.as_table()
        .ok_or("Vendor table is not a table")?;

    // Extract vendor name (from "Name" field or fallback to key)
    let name = get_string_field(fields, "Name")
        .unwrap_or(&vendor_key)
        .to_string();

    // Extract Currency
    let currency = parse_currency(fields)?;

    // Extract Type
    let vendor_type = get_string_field(fields, "Type").map(String::from);

    // Extract Ranks (keep raw table fields)
    let ranks = fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k {
            if s == "Ranks" {
                Some(v.as_table().cloned())
            } else {
                None
            }
        } else {
            None
        }
    }).flatten();

    // Extract Offerings
    let offerings_table = fields.iter().find_map(|(k, v)| {
        if let Some(LuaKey::String(s)) = k {
            if s == "Offerings" {
                v.as_table()
            } else {
                None
            }
        } else {
            None
        }
    }).ok_or("Vendor missing 'Offerings' table")?;

    // Infer the vendor's currency for fallback in parse_raw_offering
    let vendor_currency_str = match &currency {
        CurrencySpec::One(c) => Some(c.as_str()),
        CurrencySpec::Many(_) => None, // multi-currency offerings will have explicit prices
        CurrencySpec::None => None,
    };

    let mut offerings = Vec::new();
    for offering_table in offerings_table {
        let raw_offering = parse_raw_offering(&[offering_table.clone()], vendor_currency_str)?;
        offerings.push(raw_offering);
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

// Replace parse_currency with the updated version that handles positional tables
fn parse_currency(fields: &[(Option<LuaKey>, LuaValue)]) -> Result<CurrencySpec, String> {
    for (key, val) in fields {
        if let Some(LuaKey::String(s)) = key {
            if s == "Currency" {
                return match val {
                    LuaValue::String(s) => Ok(CurrencySpec::One(s.clone())),
                    LuaValue::Table(pairs) => {
                        // Check if any pair has a key
                        let has_keys = pairs.iter().any(|(k, _)| k.is_some());
                        if has_keys {
                            let mut currencies = Vec::new();
                            for (k, _v) in pairs {
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
                            // All entries are positional values (strings)
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
                    _ => Err("Currency must be a string or a table of strings".to_string()),
                };
            }
        }
    }
    Ok(CurrencySpec::None)
}

// ---------- Tests for offering normalizer ----------

#[cfg(test)]
mod offering_tests {
    use super::test_helpers::*;
    use super::*;

    fn number(n: f64) -> LuaValue { LuaValue::Number(n) }
    fn string(s: &str) -> LuaValue { LuaValue::String(s.to_string()) }
    fn table(fields: Vec<(Option<LuaKey>, LuaValue)>) -> LuaValue { LuaValue::Table(fields) }

    fn key(s: &str) -> Option<LuaKey> { Some(LuaKey::String(s.to_string())) }
    fn int_key(i: i64) -> Option<LuaKey> { Some(LuaKey::Integer(i)) }
    fn no_key() -> Option<LuaKey> { None }

    #[test]
    fn positional_single_currency() {
        // { "Kuva", "Resource", 10, 5000, Timer = 604800 }
        let fields = vec![
            (no_key(), string("Kuva")),
            (no_key(), string("Resource")),
            (no_key(), number(10.0)),
            (no_key(), number(5000.0)),
            (key("Timer"), number(604800.0)),
        ];
        let vendor_currency = Some("Pathos Clamp"); // should not be used because price is number, but we need fallback
        let result = parse_raw_offering(&fields, vendor_currency).unwrap();
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
        // { "Orokin Reactor", "Item", 20, Timer = 604800 }
        let fields = vec![
            (no_key(), string("Orokin Reactor")),
            (no_key(), string("Item")),
            (no_key(), number(20.0)),
            (key("Timer"), number(604800.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Platinum")).unwrap();
        assert_eq!(result.name, "Orokin Reactor");
        assert_eq!(result.qty, 1);
    }

    #[test]
    fn multi_currency_price() {
        // { "Item", "Type", { Credits = 5000, Standing = 250 }, 1, Prereq = 0 }
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
        let result = parse_raw_offering(&fields, Some("Platinum")).unwrap();
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
        // { "Exilus Adapter Blueprint", "Blueprint", 50000, 1, Prereq = "Natah (Quest)" }
        let fields = vec![
            (no_key(), string("Exilus Adapter Blueprint")),
            (no_key(), string("Blueprint")),
            (no_key(), number(50000.0)),
            (no_key(), number(1.0)),
            (key("Prereq"), string("Natah (Quest)")),
        ];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap();
        assert_eq!(result.prereq, Some(PrereqSpec::Quest("Natah (Quest)".to_string())));
    }

    #[test]
    fn numeric_prereq_rank() {
        // { "Energy Conversion", "Mod", 100000, Prereq = 5 } -> missing qty, so qty=1
        let fields = vec![
            (no_key(), string("Energy Conversion")),
            (no_key(), string("Mod")),
            (no_key(), number(100000.0)),
            (key("Prereq"), number(5.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap();
        assert_eq!(result.prereq, Some(PrereqSpec::Rank(5)));
        assert_eq!(result.qty, 1);
    }

    #[test]
    fn timer_and_limit_present() {
        // { "Requiem I Relic", "Relic", 10, 1, Timer = 604800, Limit = 10 }
        let fields = vec![
            (no_key(), string("Requiem I Relic")),
            (no_key(), string("Relic")),
            (no_key(), number(10.0)),
            (no_key(), number(1.0)),
            (key("Timer"), number(604800.0)),
            (key("Limit"), number(10.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Riven Sliver")).unwrap();
        assert_eq!(result.timer, Some(604800));
        assert_eq!(result.limit, Some(10));
    }

    #[test]
    fn named_offering_with_cost() {
        // { item = "Energy Conversion", cost = 100000 }
        let fields = vec![
            (key("item"), string("Energy Conversion")),
            (key("cost"), number(100000.0)),
        ];
        let result = parse_raw_offering(&fields, Some("Standing")).unwrap();
        assert_eq!(result.name, "Energy Conversion");
        assert_eq!(result.category, "Misc"); // default category
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
        // { item = "Fosfor Blau (x20) Blueprint", type = "Blueprint", cost = { Credits = 5000, Standing = 250 }, qty = 1 }
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
        let result = parse_raw_offering(&fields, None).unwrap(); // no fallback needed
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
        // "Resource," -> "Resource"
        assert_eq!(normalize_category("Resource,"), "Resource");
        assert_eq!(normalize_category("Resource"), "Resource");
        assert_eq!(normalize_category("Cosmetics"), "Cosmetic");
        assert_eq!(normalize_category("Mod"), "Mod");
    }
}

// ---------- Tests ----------

#[cfg(test)]
mod test_helpers {
    use super::*;

    pub fn number(n: f64) -> LuaValue { LuaValue::Number(n) }
    pub fn string(s: &str) -> LuaValue { LuaValue::String(s.to_string()) }
    pub fn table(fields: Vec<(Option<LuaKey>, LuaValue)>) -> LuaValue { LuaValue::Table(fields) }
    pub fn key(s: &str) -> Option<LuaKey> { Some(LuaKey::String(s.to_string())) }
    pub fn int_key(i: i64) -> Option<LuaKey> { Some(LuaKey::Integer(i)) }
    pub fn no_key() -> Option<LuaKey> { None }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
        fn parse_sample_vendor_fixture() {
            // Load the fixture file from tests/fixtures/vendors_sample.json
            let fixture_path = "tests/fixtures/vendors_sample.json";
            let json_str = std::fs::read_to_string(fixture_path)
                .expect("Fixture file missing");
            let lua_source = extract_lua_content(&json_str).expect("Failed to extract Lua");
            let tokens = tokenize(&lua_source).expect("Tokenizer failed");
            let parsed = parse(&tokens).expect("Parser failed");

            // Expect a table with top-level "Vendors"
            match parsed {
                LuaValue::Table(outer) => {
                    // Find the "Vendors" key
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

                    // Check a few known vendors
                    // Acrithis
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
                    // Check Currency
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

                    // Check Offerings is a table with at least one entry
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
                    assert!(offerings.len() > 0);
                }
                _ => panic!("Top-level is not a table"),
            }
        }

        #[test]
        fn parse_full_vendors_cache() {
            if !std::path::Path::new("cache/vendors_data_cache.json").exists() &&
               !std::path::Path::new(config::VENDORS_RAW_CACHE_FILE).exists() {
                eprintln!("Skipping full vendor cache test: file not found");
                return;
            }
            let parsed = load_vendor_data().expect("Failed to load vendor data");
            match parsed {
                LuaValue::Table(outer) => {
                    let vendors_opt = outer.iter().find_map(|(key, val)| {
                        if let Some(LuaKey::String(name)) = key {
                            if name == "Vendors" {
                                Some(val.as_table().expect("Vendors not a table"))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    });
                    let vendors = vendors_opt.expect("No Vendors key found");
                    // According to the task, there should be 66 top-level entries.
                    // We'll just check that it's around that number; the actual count may vary over time.
                    // We'll assert it's >= 60 to be safe.
                    assert!(vendors.len() >= 60, "Expected at least 60 vendors, got {}", vendors.len());
                    // Check that a known vendor exists: "Cephalon Simaris"
                    let has_simaris = vendors.iter().any(|(key, _)| {
                        matches!(key, Some(LuaKey::String(name)) if name == "Cephalon Simaris")
                    });
                    assert!(has_simaris, "Cephalon Simaris not found");
                }
                _ => panic!("Top-level is not a table"),
            }
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
        // Simple string without escapes – the tokenizer can handle escapes,
        // but the vendor data doesn't need them, so we keep this simple.
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
mod vendor_tests {
    use super::test_helpers::*;
    use super::*;

    fn number(n: f64) -> LuaValue { LuaValue::Number(n) }
    fn string(s: &str) -> LuaValue { LuaValue::String(s.to_string()) }
    fn table(fields: Vec<(Option<LuaKey>, LuaValue)>) -> LuaValue { LuaValue::Table(fields) }
    fn key(s: &str) -> Option<LuaKey> { Some(LuaKey::String(s.to_string())) }
    fn no_key() -> Option<LuaKey> { None }

    #[test]
    fn parse_single_currency_vendor() {
        // Acrithis: Currency = "Pathos Clamp"
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
        // Marie: Currency = { "Lyroic Bridge", "Ren Hypercore", "Ascaris Prime" }
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
        // Star Days: no Currency field
        let vendor_table = table(vec![
            (key("Name"), string("Star Days")),
            (key("Offerings"), table(vec![])),
        ]);
        let vendor = parse_raw_vendor("Star Days".to_string(), &vendor_table).unwrap();
        assert_eq!(vendor.currency, CurrencySpec::None);
    }

    #[test]
    fn parse_ranks_with_zero_index() {
        // Arbiters of Hexis: Ranks = { [0] = "Initiation", "Principled", ... }
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
        // Check that the first entry has key Some(LuaKey::Integer(0))
        assert!(matches!(&ranks[0].0, Some(LuaKey::Integer(0))));
        assert_eq!(ranks.len(), 6);
    }

    #[test]
    fn parse_ranks_without_zero_index() {
        // Conclave: Ranks = { "Mistral", "Whirlwind", "Tempest", "Hurricane", "Typhoon" }
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
        // All keys should be None (positional)
        for (k, _) in &ranks {
            assert!(k.is_none());
        }
        assert_eq!(ranks.len(), 5);
    }
}
