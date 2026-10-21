// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Just enough JSON (RFC 8259) for the package index and the list of
//! installed packages: objects, arrays, strings (with `\uXXXX`), whole
//! numbers, `true`, `false`, `null`. Pure, tested on the host.

use alloc::string::String;
use alloc::vec::Vec;

/// A JSON value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A whole number (the index has no fractions).
    Number(i64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Value>),
    /// An object: keys in their order.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The number, if it is one.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The elements, if it is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The JSON text (compact).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        write(self, &mut out, 0);
        out
    }
}

fn write(v: &Value, out: &mut String, depth: usize) {
    let indent = |out: &mut String, d: usize| {
        out.push('\n');
        for _ in 0..d {
            out.push(' ');
        }
    };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&alloc::format!("{n}")),
        Value::Str(s) => write_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(e, out, depth + 1);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, e)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                indent(out, depth + 1);
                write_str(k, out);
                out.push_str(": ");
                write(e, out, depth + 1);
            }
            if !m.is_empty() {
                indent(out, depth);
            }
            out.push('}');
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Why a text is not JSON (the byte offset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError(pub usize);

/// Nesting deeper than this is refused (no unbounded recursion).
const MAX_DEPTH: usize = 32;

/// Parses a whole JSON text.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    let b = text.as_bytes();
    let mut at = 0;
    let v = value(b, &mut at, 0)?;
    skip_ws(b, &mut at);
    if at != b.len() {
        return Err(ParseError(at));
    }
    Ok(v)
}

fn skip_ws(b: &[u8], at: &mut usize) {
    while *at < b.len() && matches!(b[*at], b' ' | b'\t' | b'\n' | b'\r') {
        *at += 1;
    }
}

fn value(b: &[u8], at: &mut usize, depth: usize) -> Result<Value, ParseError> {
    if depth > MAX_DEPTH {
        return Err(ParseError(*at));
    }
    skip_ws(b, at);
    let Some(&c) = b.get(*at) else {
        return Err(ParseError(*at));
    };
    match c {
        b'{' => {
            *at += 1;
            let mut members = Vec::new();
            skip_ws(b, at);
            if b.get(*at) == Some(&b'}') {
                *at += 1;
                return Ok(Value::Object(members));
            }
            loop {
                skip_ws(b, at);
                let key = string(b, at)?;
                skip_ws(b, at);
                if b.get(*at) != Some(&b':') {
                    return Err(ParseError(*at));
                }
                *at += 1;
                let v = value(b, at, depth + 1)?;
                members.push((key, v));
                skip_ws(b, at);
                match b.get(*at) {
                    Some(b',') => *at += 1,
                    Some(b'}') => {
                        *at += 1;
                        return Ok(Value::Object(members));
                    }
                    _ => return Err(ParseError(*at)),
                }
            }
        }
        b'[' => {
            *at += 1;
            let mut items = Vec::new();
            skip_ws(b, at);
            if b.get(*at) == Some(&b']') {
                *at += 1;
                return Ok(Value::Array(items));
            }
            loop {
                items.push(value(b, at, depth + 1)?);
                skip_ws(b, at);
                match b.get(*at) {
                    Some(b',') => *at += 1,
                    Some(b']') => {
                        *at += 1;
                        return Ok(Value::Array(items));
                    }
                    _ => return Err(ParseError(*at)),
                }
            }
        }
        b'"' => string(b, at).map(Value::Str),
        b't' if b[*at..].starts_with(b"true") => {
            *at += 4;
            Ok(Value::Bool(true))
        }
        b'f' if b[*at..].starts_with(b"false") => {
            *at += 5;
            Ok(Value::Bool(false))
        }
        b'n' if b[*at..].starts_with(b"null") => {
            *at += 4;
            Ok(Value::Null)
        }
        b'-' | b'0'..=b'9' => {
            let start = *at;
            *at += 1;
            while *at < b.len() && b[*at].is_ascii_digit() {
                *at += 1;
            }
            core::str::from_utf8(&b[start..*at])
                .ok()
                .and_then(|s| s.parse().ok())
                .map(Value::Number)
                .ok_or(ParseError(start))
        }
        _ => Err(ParseError(*at)),
    }
}

fn hex4(b: &[u8], at: usize) -> Option<u32> {
    let s = core::str::from_utf8(b.get(at..at + 4)?).ok()?;
    u32::from_str_radix(s, 16).ok()
}

fn string(b: &[u8], at: &mut usize) -> Result<String, ParseError> {
    if b.get(*at) != Some(&b'"') {
        return Err(ParseError(*at));
    }
    *at += 1;
    let mut out: Vec<u8> = Vec::new();
    loop {
        let Some(&c) = b.get(*at) else {
            return Err(ParseError(*at));
        };
        *at += 1;
        match c {
            b'"' => break,
            b'\\' => {
                let Some(&e) = b.get(*at) else {
                    return Err(ParseError(*at));
                };
                *at += 1;
                let ch = match e {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'/' => '/',
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    b'u' => {
                        let mut code = hex4(b, *at).ok_or(ParseError(*at))?;
                        *at += 4;
                        if (0xD800..0xDC00).contains(&code) && b[*at..].starts_with(b"\\u") {
                            let low = hex4(b, *at + 2).ok_or(ParseError(*at))?;
                            *at += 6;
                            code = 0x10000
                                + ((code - 0xD800) << 10)
                                + (low.wrapping_sub(0xDC00) & 0x3FF);
                        }
                        char::from_u32(code).unwrap_or('\u{FFFD}')
                    }
                    _ => return Err(ParseError(*at)),
                };
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            c if c < 0x20 => return Err(ParseError(*at)),
            c => out.push(c),
        }
    }
    String::from_utf8(out).map_err(|_| ParseError(*at))
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(String::from(text))
}

/// An object from pairs.
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (String::from(k), v))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trip() {
        let text = r#"{"format": 1, "packages": [{"name": "lua", "size": 593920,
            "depends": [], "ok": true, "x": null, "d": "tab\there \"q\" é 😀"}]}"#;
        let v = parse(text).unwrap();
        assert_eq!(v.get("format").and_then(Value::as_i64), Some(1));
        let p = &v.get("packages").unwrap().as_array().unwrap()[0];
        assert_eq!(p.get("name").and_then(Value::as_str), Some("lua"));
        assert_eq!(
            p.get("d").and_then(Value::as_str),
            Some("tab\there \"q\" é 😀")
        );
        assert_eq!(p.get("depends"), Some(&Value::Array(vec![])));
        let again = parse(&v.to_text()).unwrap();
        assert_eq!(again, v);
    }

    #[test]
    fn errors() {
        assert!(parse("").is_err());
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("{\"a\" 1}").is_err());
        assert!(parse("\"\x01\"").is_err());
        assert!(parse("1 2").is_err());
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(parse(&deep).is_err());
        assert_eq!(parse(" -12 "), Ok(Value::Number(-12)));
        assert_eq!(object(vec![("a", s("b"))]).to_text(), "{\n \"a\": \"b\"\n}");
    }
}
