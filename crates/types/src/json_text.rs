//! Lossless JSON → [`Value`] parser.
//!
//! `serde_json` (without `arbitrary_precision`) routes fractional numbers through `f64`, which
//! silently changes values such as `10.10` → `10.1` or rounds long decimals. Migration data must
//! not change, so numbers are kept as exact text: integers that fit `i64` become `Int`, numbers
//! with a fraction become `Decimal`, and only numbers with exponents become `Float`.

use std::collections::BTreeMap;

use crate::error::{MaError, MaResult};
use crate::value::Value;

const MAX_DEPTH: usize = 256;

pub fn parse(bytes: &[u8]) -> MaResult<Value> {
    let mut p = Parser { b: bytes, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

/// Serialize a value as plain JSON, writing decimals as exact number text.
pub fn write_plain(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Decimal(d) if crate::value::is_decimal_text(d) || d.parse::<i128>().is_ok() => {
            let d = crate::value::normalize_decimal(d);
            out.extend_from_slice(d.as_bytes())
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_plain(item, out);
            }
            out.push(b']');
        }
        Value::Object(m) => write_object(m.iter(), out),
        other => {
            serde_json::to_writer(&mut *out, &other.to_plain_json()).expect("vec write");
        }
    }
}

/// Serialize `(key, value)` pairs as a JSON object.
pub fn write_object<'a>(fields: impl Iterator<Item = (&'a String, &'a Value)>, out: &mut Vec<u8>) {
    out.push(b'{');
    for (i, (k, v)) in fields.enumerate() {
        if i > 0 {
            out.push(b',');
        }
        serde_json::to_writer(&mut *out, k).expect("vec write");
        out.push(b':');
        write_plain(v, out);
    }
    out.push(b'}');
}

pub fn to_plain_string(v: &Value) -> String {
    let mut out = vec![];
    write_plain(v, &mut out);
    String::from_utf8(out).expect("utf8")
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, m: &str) -> MaError {
        MaError::validation(format!("invalid JSON at byte {}: {m}", self.i))
    }
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn expect_lit(&mut self, lit: &[u8], v: Value) -> MaResult<Value> {
        if self.b[self.i..].starts_with(lit) {
            self.i += lit.len();
            Ok(v)
        } else {
            Err(self.err("unexpected token"))
        }
    }
    fn value(&mut self, depth: usize) -> MaResult<Value> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Value::Object(m));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"') {
                        return Err(self.err("expected object key"));
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.peek() != Some(b':') {
                        return Err(self.err("expected ':'"));
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    m.insert(k, v);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Object(m));
                        }
                        _ => return Err(self.err("expected ',' or '}'")),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = vec![];
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Value::Array(a));
                }
                loop {
                    self.ws();
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Array(a));
                        }
                        _ => return Err(self.err("expected ',' or ']'")),
                    }
                }
            }
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.expect_lit(b"true", Value::Bool(true)),
            Some(b'f') => self.expect_lit(b"false", Value::Bool(false)),
            Some(b'n') => self.expect_lit(b"null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(self.err("unexpected character")),
        }
    }
    fn number(&mut self) -> MaResult<Value> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let int_start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == int_start || (self.i - int_start > 1 && self.b[int_start] == b'0') {
            return Err(self.err("invalid number"));
        }
        let mut frac = false;
        let mut exp = false;
        if self.peek() == Some(b'.') {
            frac = true;
            self.i += 1;
            let s = self.i;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
            if self.i == s {
                return Err(self.err("invalid fraction"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            exp = true;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            let s = self.i;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
            if self.i == s {
                return Err(self.err("invalid exponent"));
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).expect("ascii");
        if exp {
            return text.parse::<f64>().map(Value::Float).map_err(|_| self.err("invalid number"));
        }
        if !frac && let Ok(i) = text.parse::<i64>() {
            return Ok(Value::Int(i));
        }
        Ok(Value::Decimal(text.to_string()))
    }
    fn hex4(&mut self) -> MaResult<u32> {
        let s = self.b.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u escape"))?;
        let v = u32::from_str_radix(std::str::from_utf8(s).map_err(|_| self.err("bad \\u"))?, 16).map_err(|_| self.err("bad \\u"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> MaResult<String> {
        self.i += 1; // opening quote
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.peek().ok_or_else(|| self.err("bad escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp) {
                                if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err(self.err("lone surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("bad surrogate pair"));
                                }
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            }
                            let ch = char::from_u32(cp).ok_or_else(|| self.err("invalid code point"))?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                c if c < 0x20 => return Err(self.err("control character in string")),
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("invalid UTF-8 in string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_numbers() {
        let v = parse(br#"{"a": 10.10, "b": 12345678901234567890.123456789, "c": 7, "d": 1e3, "e": -0.50, "f": 99999999999999999999}"#)
            .unwrap();
        let Value::Object(m) = v else { panic!() };
        assert_eq!(m["a"], Value::Decimal("10.10".into()));
        assert_eq!(m["b"], Value::Decimal("12345678901234567890.123456789".into()));
        assert_eq!(m["c"], Value::Int(7));
        assert_eq!(m["d"], Value::Float(1000.0));
        assert_eq!(m["e"], Value::Decimal("-0.50".into()));
        assert_eq!(m["f"], Value::Decimal("99999999999999999999".into()));
    }

    #[test]
    fn strings_and_structure() {
        let bs = '\\';
        let input = format!(r#"[{{"s":"a{bs}"b{bs}{bs}c{bs}u00e9{bs}ud83d{bs}ude00{bs}n"}}, [], {{}}, null, true]"#);
        let v = parse(input.as_bytes()).unwrap();
        let Value::Array(a) = v else { panic!() };
        assert_eq!(a[0].get_path(&["s"]), Some(&Value::String("a\"b\\cé😀\n".into())));
        assert_eq!(a[1], Value::Array(vec![]));
        assert_eq!(a[3], Value::Null);
        assert!(parse(b"{\"a\":}").is_err());
        assert!(parse(b"[1,]").is_err());
        assert!(parse(b"01").is_err());
    }

    #[test]
    fn agrees_with_serde_json_on_structure() {
        let docs = [r#"{"x":[1,2,{"y":"z"}],"n":null}"#, r#"[true,false,"\t"]"#];
        for d in docs {
            let ours = parse(d.as_bytes()).unwrap().to_plain_json();
            let theirs: serde_json::Value = serde_json::from_str(d).unwrap();
            assert_eq!(ours, theirs);
        }
    }
}
