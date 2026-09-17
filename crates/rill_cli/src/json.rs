//! Just enough JSON for the language server to talk.
//!
//! The rest of this toolchain has one dependency, and it is LLVM. A protocol
//! whose messages are a handful of known shapes did not seem worth a second
//! one, so this is a value, a parser and an escaper — about as much JSON as
//! `initialize` and `textDocument/hover` need between them.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    /// `j["a"]["b"]` without the panicking: anything missing is `Null`.
    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(m) => m.get(key).unwrap_or(&Json::Null),
            _ => &Json::Null,
        }
    }

    pub fn at(&self, i: usize) -> &Json {
        match self {
            Json::Arr(v) => v.get(i).unwrap_or(&Json::Null),
            _ => &Json::Null,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_usize(&self) -> Option<usize> {
        match self {
            Json::Num(n) if *n >= 0.0 => Some(*n as usize),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// One more field on an object; nothing on anything else.
    pub fn push(&mut self, key: &str, value: Json) {
        if let Json::Obj(m) = self {
            m.insert(key.to_string(), value);
        }
    }

    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    pub fn num(n: usize) -> Json {
        Json::Num(n as f64)
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 9e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Json::Str(s) => escape(s, out),
            Json::Arr(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    escape(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn escape(s: &str, out: &mut String) {
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

pub fn parse(src: &str) -> Option<Json> {
    let mut p = Parser { b: src.as_bytes(), i: 0 };
    p.ws();
    let v = p.value()?;
    Some(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.i < self.b.len() && self.b[self.i] == c {
            self.i += 1;
            self.ws();
            return true;
        }
        false
    }

    fn value(&mut self) -> Option<Json> {
        let c = *self.b.get(self.i)?;
        let v = match c {
            b'{' => {
                self.i += 1;
                self.ws();
                let mut m = BTreeMap::new();
                if !self.eat(b'}') {
                    loop {
                        let k = match self.value()? {
                            Json::Str(s) => s,
                            _ => return None,
                        };
                        if !self.eat(b':') {
                            return None;
                        }
                        m.insert(k, self.value()?);
                        if self.eat(b',') {
                            continue;
                        }
                        if self.eat(b'}') {
                            break;
                        }
                        return None;
                    }
                }
                Json::Obj(m)
            }
            b'[' => {
                self.i += 1;
                self.ws();
                let mut v = Vec::new();
                if !self.eat(b']') {
                    loop {
                        v.push(self.value()?);
                        if self.eat(b',') {
                            continue;
                        }
                        if self.eat(b']') {
                            break;
                        }
                        return None;
                    }
                }
                Json::Arr(v)
            }
            b'"' => Json::Str(self.string()?),
            b't' => {
                self.word("true")?;
                Json::Bool(true)
            }
            b'f' => {
                self.word("false")?;
                Json::Bool(false)
            }
            b'n' => {
                self.word("null")?;
                Json::Null
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len()
                    && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                {
                    self.i += 1;
                }
                let text = std::str::from_utf8(&self.b[start..self.i]).ok()?;
                Json::Num(text.parse().ok()?)
            }
        };
        self.ws();
        Some(v)
    }

    fn word(&mut self, w: &str) -> Option<()> {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            return Some(());
        }
        None
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1; // the opening quote
        let mut s = String::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(s),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let n = u32::from_str_radix(hex, 16).ok()?;
                            // A surrogate pair is two escapes; the second is
                            // read here so that anything above the basic
                            // plane survives the trip.
                            let ch = if (0xD800..0xDC00).contains(&n) {
                                if self.b.get(self.i) != Some(&b'\\') {
                                    return None;
                                }
                                self.i += 2; // `\u`
                                let hex2 =
                                    std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                                self.i += 4;
                                let low = u32::from_str_radix(hex2, 16).ok()?;
                                char::from_u32(0x10000 + ((n - 0xD800) << 10) + (low - 0xDC00))?
                            } else {
                                char::from_u32(n)?
                            };
                            s.push(ch);
                        }
                        _ => return None,
                    }
                }
                _ => {
                    // Multi-byte UTF-8 goes through as it stands.
                    let start = self.i - 1;
                    let len = match c {
                        0x00..=0x7f => 1,
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    };
                    self.i = start + len;
                    s.push_str(std::str::from_utf8(self.b.get(start..self.i)?).ok()?);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_what_the_protocol_sends() {
        let src = r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover",
                      "params":{"textDocument":{"uri":"file:///a b/x.rill"},
                                "position":{"line":3,"character":11}}}"#;
        let j = parse(src).expect("parses");
        assert_eq!(j.get("method").as_str(), Some("textDocument/hover"));
        assert_eq!(j.get("id").as_usize(), Some(1));
        assert_eq!(
            j.get("params").get("textDocument").get("uri").as_str(),
            Some("file:///a b/x.rill")
        );
        assert_eq!(j.get("params").get("position").get("character").as_usize(), Some(11));
        // Anything missing is Null rather than a panic.
        assert!(j.get("params").get("nope").get("deeper").is_null());
    }

    #[test]
    fn escapes_and_unescapes() {
        let s = "a \"quote\", a \\ and a \n — and a ü";
        let j = Json::str(s);
        let back = parse(&j.render()).expect("parses");
        assert_eq!(back.as_str(), Some(s));
        assert_eq!(parse(r#""ü""#).unwrap().as_str(), Some("ü"));
        assert_eq!(parse(r#""😀""#).unwrap().as_str(), Some("😀"));
    }

    #[test]
    fn numbers_come_back_whole() {
        assert_eq!(Json::num(0).render(), "0");
        assert_eq!(Json::num(42).render(), "42");
        assert_eq!(parse("[1,2,3]").unwrap().at(2).as_usize(), Some(3));
    }
}
