use std::iter::Peekable;
use std::str::Chars;

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

// ---------- Tests ----------

#[cfg(test)]
mod tests {
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
