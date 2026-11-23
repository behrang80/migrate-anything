//! Canonical, lossless value representation.
//!
//! A *missing* field is represented by the absence of a key in [`crate::Record::fields`]; it is
//! never conflated with [`Value::Null`], an empty string or an empty array.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Logical field types understood by the mapping runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Int,
    Float,
    Decimal,
    Bool,
    Date,
    DateTime,
    Bytes,
    Array,
    Object,
    /// The type could not be determined (e.g. a column that is always null).
    Unknown,
}

impl FieldType {
    /// Whether a value of type `self` can be converted to `to` without inventing information.
    pub fn converts_to(self, to: FieldType) -> bool {
        use FieldType::*;
        if self == to || to == String || self == Unknown || to == Unknown {
            return true;
        }
        matches!(
            (self, to),
            (Int, Float)
                | (Int, Decimal)
                | (Float, Decimal)
                | (Decimal, Float)
                | (Int, Bool)
                | (Bool, Int)
                | (Date, DateTime)
                | (DateTime, Date)
                | (String, Int)
                | (String, Float)
                | (String, Decimal)
                | (String, Bool)
                | (String, Date)
                | (String, DateTime)
        )
    }
}

/// A single canonical value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// An explicit null in the source.
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact decimal kept as its canonical text (never routed through `f64`).
    Decimal(String),
    String(String),
    /// Binary content, base64 encoded in serialized form.
    Bytes(String),
    Date(NaiveDate),
    DateTime(DateTime<FixedOffset>),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
    /// A value that was present but could not be parsed as its declared type.
    Invalid {
        raw: String,
        reason: String,
    },
    /// A value of a type the platform cannot represent; the raw text is preserved.
    Unsupported {
        type_name: String,
        raw: String,
    },
}

impl Value {
    pub fn field_type(&self) -> FieldType {
        match self {
            Value::Null | Value::Invalid { .. } | Value::Unsupported { .. } => FieldType::Unknown,
            Value::Bool(_) => FieldType::Bool,
            Value::Int(_) => FieldType::Int,
            Value::Float(_) => FieldType::Float,
            Value::Decimal(_) => FieldType::Decimal,
            Value::String(_) => FieldType::String,
            Value::Bytes(_) => FieldType::Bytes,
            Value::Date(_) => FieldType::Date,
            Value::DateTime(_) => FieldType::DateTime,
            Value::Array(_) => FieldType::Array,
            Value::Object(_) => FieldType::Object,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_problem(&self) -> bool {
        matches!(self, Value::Invalid { .. } | Value::Unsupported { .. })
    }

    /// Text form used for identifiers, CSV output and comparisons.
    pub fn to_text(&self) -> Option<String> {
        Some(match self {
            Value::Null => return None,
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Decimal(d) => d.clone(),
            Value::String(s) => s.clone(),
            Value::Bytes(b) => b.clone(),
            Value::Date(d) => d.format("%Y-%m-%d").to_string(),
            Value::DateTime(dt) => dt.to_rfc3339(),
            Value::Array(_) | Value::Object(_) => self.to_plain_json().to_string(),
            Value::Invalid { raw, .. } => raw.clone(),
            Value::Unsupported { raw, .. } => raw.clone(),
        })
    }

    /// Convert to ordinary JSON for output to JSON files and HTTP APIs.
    pub fn to_plain_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::Float(f) => serde_json::Number::from_f64(*f).map(J::Number).unwrap_or(J::Null),
            Value::Decimal(d) => {
                // Emit decimals as JSON numbers only when that is exact in text form.
                match serde_json::from_str::<serde_json::Number>(d) {
                    Ok(n) => J::Number(n),
                    Err(_) => J::String(d.clone()),
                }
            }
            Value::String(s) | Value::Bytes(s) => J::String(s.clone()),
            Value::Date(_) | Value::DateTime(_) => J::String(self.to_text().unwrap_or_default()),
            Value::Array(items) => J::Array(items.iter().map(Value::to_plain_json).collect()),
            Value::Object(map) => J::Object(map.iter().map(|(k, v)| (k.clone(), v.to_plain_json())).collect()),
            Value::Invalid { raw, .. } | Value::Unsupported { raw, .. } => J::String(raw.clone()),
        }
    }

    /// Convert ordinary JSON into a canonical value. Numbers with a fraction become `Decimal`
    /// when `exact_numbers` is set, so currency amounts are not rounded through `f64`.
    pub fn from_plain_json(v: &serde_json::Value, exact_numbers: bool) -> Value {
        use serde_json::Value as J;
        match v {
            J::Null => Value::Null,
            J::Bool(b) => Value::Bool(*b),
            J::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if exact_numbers {
                    Value::Decimal(n.to_string())
                } else {
                    n.as_f64().map(Value::Float).unwrap_or_else(|| Value::Decimal(n.to_string()))
                }
            }
            J::String(s) => Value::String(s.clone()),
            J::Array(items) => Value::Array(items.iter().map(|i| Value::from_plain_json(i, exact_numbers)).collect()),
            J::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), Value::from_plain_json(v, exact_numbers))).collect()),
        }
    }

    /// Infer a typed value from text (CSV cells, query strings). Empty text stays an empty string.
    pub fn infer_from_text(s: &str) -> Value {
        if s.is_empty() {
            return Value::String(String::new());
        }
        if let Ok(i) = s.parse::<i64>() {
            // Keep leading-zero identifiers ("007") as strings.
            if !(s.len() > 1 && (s.starts_with('0') || s.starts_with("-0"))) {
                return Value::Int(i);
            }
            return Value::String(s.to_string());
        }
        if is_decimal_text(s) {
            return Value::Decimal(s.to_string());
        }
        match s {
            "true" | "TRUE" | "True" => return Value::Bool(true),
            "false" | "FALSE" | "False" => return Value::Bool(false),
            _ => {}
        }
        if let Some(v) = parse_temporal(s) {
            return v;
        }
        Value::String(s.to_string())
    }

    /// Parse text as the requested type, producing [`Value::Invalid`] rather than failing.
    pub fn parse_as(s: &str, ty: FieldType) -> Value {
        let invalid = |reason: &str| Value::Invalid { raw: s.to_string(), reason: reason.to_string() };
        match ty {
            FieldType::String | FieldType::Unknown => Value::String(s.to_string()),
            FieldType::Int => s.trim().parse::<i64>().map(Value::Int).unwrap_or_else(|_| invalid("not an integer")),
            FieldType::Float => s.trim().parse::<f64>().map(Value::Float).unwrap_or_else(|_| invalid("not a number")),
            FieldType::Decimal => {
                let t = s.trim();
                if t.parse::<i64>().is_ok() || is_decimal_text(t) { Value::Decimal(normalize_decimal(t)) } else { invalid("not a decimal") }
            }
            FieldType::Bool => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "t" | "yes" | "y" | "1" => Value::Bool(true),
                "false" | "f" | "no" | "n" | "0" => Value::Bool(false),
                _ => invalid("not a boolean"),
            },
            FieldType::Date => match parse_temporal(s.trim()) {
                Some(Value::Date(d)) => Value::Date(d),
                Some(Value::DateTime(dt)) => Value::Date(dt.date_naive()),
                _ => invalid("not a date"),
            },
            FieldType::DateTime => match parse_temporal(s.trim()) {
                Some(Value::DateTime(dt)) => Value::DateTime(dt),
                Some(Value::Date(d)) => Value::DateTime(d.and_hms_opt(0, 0, 0).expect("midnight").and_utc().fixed_offset()),
                _ => invalid("not a datetime"),
            },
            FieldType::Bytes => Value::Bytes(s.to_string()),
            FieldType::Array | FieldType::Object => match crate::json_text::parse(s.as_bytes()) {
                Ok(v) => v,
                Err(_) => invalid("not JSON"),
            },
        }
    }

    /// Navigate a dotted path (`address.city`, `lines.0.amount`).
    pub fn get_path(&self, path: &[&str]) -> Option<&Value> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        match self {
            Value::Object(m) => m.get(*first)?.get_path(rest),
            Value::Array(a) => a.get(first.parse::<usize>().ok()?)?.get_path(rest),
            _ => None,
        }
    }
}

fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 { format!("{f:.1}") } else { f.to_string() }
}

/// `-12.50`, `3.0`, `.5` → true; `1e5`, `abc` → false.
pub fn is_decimal_text(s: &str) -> bool {
    let s = s.strip_prefix('-').or_else(|| s.strip_prefix('+')).unwrap_or(s);
    let Some((int, frac)) = s.split_once('.') else { return false };
    !frac.is_empty()
        && int.chars().all(|c| c.is_ascii_digit())
        && frac.chars().all(|c| c.is_ascii_digit())
        && (int.len() <= 1 || !int.starts_with('0'))
}

/// Normalise decimal text: strip `+`, ensure a leading zero. Trailing zeros are kept because
/// they can carry meaning (scale) in financial data.
pub fn normalize_decimal(s: &str) -> String {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let body = if body.starts_with('.') { format!("0{body}") } else { body.to_string() };
    if neg { format!("-{body}") } else { body }
}

/// Multiply decimal text by 10^pow exactly (pow may be negative). Used for currency minor units.
pub fn scale_decimal(s: &str, pow: i32) -> Option<String> {
    let s = normalize_decimal(s);
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b.to_string()),
        None => (false, s.clone()),
    };
    let (int, frac) = body.split_once('.').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((body.clone(), String::new()));
    if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let digits = format!("{int}{frac}");
    let point = int.len() as i64 + pow as i64; // position of decimal point in `digits`
    let mut out = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point as usize >= digits.len() {
        format!("{}{}", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    };
    // strip leading zeros of integer part
    if let Some((i, f)) = out.split_once('.') {
        let i = i.trim_start_matches('0');
        out = format!("{}.{}", if i.is_empty() { "0" } else { i }, f);
    } else {
        let i = out.trim_start_matches('0');
        out = if i.is_empty() { "0".into() } else { i.to_string() };
    }
    Some(if neg && out.chars().any(|c| c.is_ascii_digit() && c != '0') { format!("-{out}") } else { out })
}

fn parse_temporal(s: &str) -> Option<Value> {
    if s.len() < 8 || !s.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(Value::Date(d));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(Value::DateTime(dt));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Value::DateTime(ndt.and_utc().fixed_offset()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_null_empty_are_distinct() {
        assert_ne!(Value::Null, Value::String(String::new()));
        assert_ne!(Value::Array(vec![]), Value::Null);
        assert_eq!(Value::infer_from_text(""), Value::String(String::new()));
    }

    #[test]
    fn leading_zero_ids_stay_strings() {
        assert_eq!(Value::infer_from_text("007"), Value::String("007".into()));
        assert_eq!(Value::infer_from_text("7"), Value::Int(7));
        assert_eq!(Value::infer_from_text("12.50"), Value::Decimal("12.50".into()));
    }

    #[test]
    fn invalid_values_keep_raw_text() {
        match Value::parse_as("abc", FieldType::Int) {
            Value::Invalid { raw, .. } => assert_eq!(raw, "abc"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn decimal_scaling_is_exact() {
        assert_eq!(scale_decimal("12.34", 2).unwrap(), "1234");
        assert_eq!(scale_decimal("1234", -2).unwrap(), "12.34");
        assert_eq!(scale_decimal("0.1", 2).unwrap(), "10");
        assert_eq!(scale_decimal("-5", -3).unwrap(), "-0.005");
        assert_eq!(scale_decimal("19.999", 2).unwrap(), "1999.9");
    }

    #[test]
    fn serde_roundtrip_preserves_types() {
        let v = Value::Object(
            [
                ("a".to_string(), Value::Decimal("1.10".into())),
                ("b".to_string(), Value::Null),
                ("c".to_string(), Value::Invalid { raw: "x".into(), reason: "r".into() }),
            ]
            .into_iter()
            .collect(),
        );
        let s = serde_json::to_string(&v).unwrap();
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    proptest::proptest! {
        #[test]
        fn scale_roundtrip(int in 0u64..1_000_000_000, frac in 0u32..100, pow in 0i32..6) {
            let s = format!("{int}.{frac:02}");
            let up = scale_decimal(&s, pow).unwrap();
            let down = scale_decimal(&up, -pow).unwrap();
            // compare numerically via trimmed representation
            let norm = |x: &str| {
                let x = if x.contains('.') { x.trim_end_matches('0').trim_end_matches('.') } else { x };
                x.to_string()
            };
            proptest::prop_assert_eq!(norm(&down), norm(&scale_decimal(&s, 0).unwrap()));
        }
    }
}
