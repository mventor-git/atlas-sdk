//! Minimal JSON codec over the existing [`Value`] type.
//!
//! The plugin protocol is language-independent, so it cannot be Rust types.
//! JSON is the least-common-denominator encoding that every target language
//! already has. Rather than take a serde dependency, this is the whole codec
//! we need: parse to `Value`, and render `Value` back out.
//!
//! ponytail: hand-rolled JSON. Ceiling: no unicode escapes beyond \uXXXX, no
//! exponents beyond the standard form, no streaming. Upgrade to serde_json only
//! if a real plugin needs those.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct JsonError(pub String);

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for JsonError {}

fn err<T>(msg: impl Into<String>) -> Result<T, JsonError> {
    Err(JsonError(msg.into()))
}

/// Render a `Value` as JSON. Object keys are emitted in `BTreeMap` order, so
/// the output is byte-stable for a given value — which matters because the
/// protocol is asserted against exact strings.
pub fn to_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Num(n) => {
            if n.is_finite() {
                let _ = write!(out, "{n}");
            } else {
                // JSON has no infinity or NaN.
                out.push_str("null");
            }
        }
        Value::Str(s) => write_string(s, out),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Map(entries) => {
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write_value(v, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse JSON text into a `Value`.
pub fn parse_json(text: &str) -> Result<Value, JsonError> {
    let bytes: Vec<char> = text.chars().collect();
    let mut p = Parser {
        chars: &bytes,
        pos: 0,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
        return err(format!("trailing characters at position {}", p.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, c: char) -> Result<(), JsonError> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            err(format!("expected '{c}' at position {}", self.pos))
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), JsonError> {
        for expected in word.chars() {
            if self.bump() != Some(expected) {
                return err(format!("invalid literal at position {}", self.pos));
            }
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Value, JsonError> {
        match self.peek() {
            None => err("unexpected end of input"),
            Some('n') => {
                self.literal("null")?;
                Ok(Value::Null)
            }
            Some('t') => {
                self.literal("true")?;
                Ok(Value::Bool(true))
            }
            Some('f') => {
                self.literal("false")?;
                Ok(Value::Bool(false))
            }
            Some('"') => self.string().map(Value::Str),
            Some('[') => self.array(),
            Some('{') => self.object(),
            Some(_) => self.number(),
        }
    }

    fn array(&mut self) -> Result<Value, JsonError> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Value::List(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => return Ok(Value::List(items)),
                _ => return err(format!("expected ',' or ']' at position {}", self.pos)),
            }
        }
    }

    fn object(&mut self) -> Result<Value, JsonError> {
        self.expect('{')?;
        let mut entries: BTreeMap<String, Value> = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Value::Map(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(':')?;
            self.skip_ws();
            let value = self.value()?;
            entries.insert(key, value);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => return Ok(Value::Map(entries)),
                _ => return err(format!("expected ',' or '}}' at position {}", self.pos)),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return err("unterminated string"),
                Some('"') => return Ok(out),
                Some('\\') => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some('u') => {
                        let hex: String = (0..4).filter_map(|_| self.bump()).collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| JsonError(format!("bad unicode escape \\u{hex}")))?;
                        out.push(
                            char::from_u32(code)
                                .ok_or_else(|| JsonError(format!("bad code point {code}")))?,
                        );
                    }
                    _ => return err("invalid escape"),
                },
                Some(c) => out.push(c),
            }
        }
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some('.') {
            self.pos += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if start == self.pos {
            return err(format!("expected a value at position {start}"));
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<f64>()
            .map(Value::Num)
            .map_err(|_| JsonError(format!("invalid number '{text}'")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_stable_output() {
        let v = Value::map(vec![
            ("b", Value::num(2)),
            ("a", Value::str("x")),
            ("z", Value::List(vec![Value::Bool(true), Value::Null])),
        ]);
        // Keys sorted, so the encoding is byte-stable and assertable.
        assert_eq!(to_json(&v), r#"{"a":"x","b":2,"z":[true,null]}"#);
    }

    #[test]
    fn escapes_control_characters_and_quotes() {
        let v = Value::str("a\"b\\c\nd\te");
        assert_eq!(to_json(&v), r#""a\"b\\c\nd\te""#);
    }

    #[test]
    fn round_trips() {
        let text = r#"{"a":[1,2.5,-3],"b":{"c":null},"d":"e"}"#;
        assert_eq!(to_json(&parse_json(text).unwrap()), text);
    }

    #[test]
    fn parses_escapes() {
        let v = parse_json(r#""a\nbA""#).unwrap();
        assert_eq!(v, Value::str("a\nbA"));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["{", "[1,]", "\"unterminated", "{\"a\":}", "tru", "{} {}"] {
            assert!(parse_json(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_json("{} x").is_err());
    }
}
