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

/// Tokenizer for Lua source, specialized for the Module:Vendors/data format.
/// Handles `--` line comments, single/double quoted strings, and numbers.
pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();

    while i < chars.len() {
        let ch = chars[i];
        match ch {
            // Whitespace
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
            }
            // Comments: `--` to end of line
            '-' => {
                if i + 1 < chars.len() && chars[i + 1] == '-' {
                    i += 2;
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                } else {
                    // Negative number
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
            // Quoted strings
            '"' | '\'' => {
                let quote = ch;
                i += 1; // consume opening quote
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
                        i += 1; // consume backslash
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
                            i += 1; // consume escaped character
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
            // Numbers
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
            // Identifiers
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut ident = String::new();
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    ident.push(chars[i]);
                    i += 1;
                }
                tokens.push(Token::Ident(ident));
            }
            // Symbols
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
