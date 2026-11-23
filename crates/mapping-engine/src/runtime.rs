//! Deterministic transformation runtime. Same input + same spec + same id map = same output.

use std::collections::{BTreeMap, BTreeSet};

use ma_types::*;

/// Resolves a reference: `(source object, source id) -> destination id`.
pub trait RefLookup {
    fn resolve(&self, object: &str, source_id: &str) -> Option<String>;
}

impl<F: Fn(&str, &str) -> Option<String>> RefLookup for F {
    fn resolve(&self, object: &str, source_id: &str) -> Option<String> {
        self(object, source_id)
    }
}

/// Executes sandboxed extensions (implemented by `ma-sandbox`).
pub trait ExtensionRunner: Send + Sync {
    fn call(&self, name: &str, input: &Value) -> MaResult<Value>;
}

pub struct EvalContext<'a> {
    pub refs: &'a dyn RefLookup,
    pub extensions: Option<&'a dyn ExtensionRunner>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransformedRecord {
    pub fields: BTreeMap<String, Value>,
    /// At least one non-trivial transformation produced a value.
    pub transformed: bool,
    /// `(source path, original value, reason)` preserved in the exception archive.
    pub archived: Vec<(String, Value, String)>,
    /// Record-level errors (the record must not be written).
    pub errors: Vec<String>,
    /// Values the destination cannot represent.
    pub unsupported: Vec<String>,
}

enum Eval {
    Missing,
    Val(Value),
}

/// Whether an expression is a plain copy (counts as "migrated natively").
pub fn is_native(expr: &Expr) -> bool {
    match expr {
        Expr::Field { .. } => true,
        Expr::Cast { input, .. } => matches!(**input, Expr::Field { .. }),
        _ => false,
    }
}

/// Top-level source field names read by a mapping.
pub fn consumed_fields(om: &ObjectMapping) -> BTreeSet<String> {
    om.source_paths().into_iter().map(|p| p.split('.').next().unwrap_or("").to_string()).collect()
}

/// Transform one record. `skip_deferred` omits fields written in the second pass.
pub fn transform_record(
    om: &ObjectMapping,
    target: Option<&ObjectDef>,
    consumed: &BTreeSet<String>,
    record: &Record,
    ctx: &EvalContext<'_>,
    skip_deferred: bool,
) -> TransformedRecord {
    let mut out = TransformedRecord::default();
    for fm in &om.fields {
        if fm.status == ReviewStatus::Rejected || (skip_deferred && fm.deferred) {
            continue;
        }
        let mut archived = vec![];
        match eval(&fm.expr, record, ctx, &mut archived) {
            Eval::Missing => {}
            Eval::Val(v) => match v {
                Value::Invalid { raw, reason } => out.errors.push(format!("field `{}`: invalid value `{raw}` ({reason})", fm.target)),
                Value::Unsupported { type_name, .. } => {
                    out.unsupported.push(format!("field `{}`: unsupported type {type_name}", fm.target))
                }
                v => {
                    if !is_native(&fm.expr) {
                        out.transformed = true;
                    }
                    out.fields.insert(fm.target.clone(), v);
                }
            },
        }
        out.archived.extend(archived);
    }
    if let Some(def) = target {
        for f in &def.fields {
            let deferred = om.fields.iter().any(|m| m.target == f.name && m.deferred && skip_deferred);
            if f.required && !f.read_only && !deferred && matches!(out.fields.get(&f.name), None | Some(Value::Null)) {
                out.errors.push(format!("required destination field `{}` has no value", f.name));
            }
        }
    }
    if om.unmapped_fields == UnmappedFieldPolicy::Archive {
        for (k, v) in &record.fields {
            if !consumed.contains(k) && !om.skip_fields.contains(k) {
                out.archived.push((k.clone(), v.clone(), "source field has no destination mapping".into()));
            }
        }
    }
    out
}

fn eval(expr: &Expr, r: &Record, ctx: &EvalContext<'_>, archived: &mut Vec<(String, Value, String)>) -> Eval {
    use Eval::*;
    match expr {
        Expr::Field { path } => match r.get_path(path) {
            Some(v) => Val(v.clone()),
            None => Missing,
        },
        Expr::Const { value } => Val(value.clone()),
        Expr::Cast { input, to } => match eval(input, r, ctx, archived) {
            Val(v) if !v.is_null() => Val(cast_value(&v, *to)),
            other => other,
        },
        Expr::Concat { parts, separator } => {
            let texts: Vec<String> = parts
                .iter()
                .filter_map(|p| match eval(p, r, ctx, archived) {
                    Val(v) => v.to_text().filter(|s| !s.is_empty()),
                    Missing => None,
                })
                .collect();
            if texts.is_empty() {
                let all_missing = parts.iter().all(|p| matches!(eval(p, r, ctx, &mut vec![]), Missing));
                if all_missing { Missing } else { Val(Value::Null) }
            } else {
                Val(Value::String(texts.join(separator)))
            }
        }
        Expr::Split { input, separator, index } => match eval(input, r, ctx, archived) {
            Val(v) => match v.to_text() {
                Some(t) => Val(t.split(separator.as_str()).nth(*index).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null)),
                None => Val(Value::Null),
            },
            Missing => Missing,
        },
        Expr::EnumMap { input, map, on_unmapped } => match eval(input, r, ctx, archived) {
            Val(Value::Null) => Val(Value::Null),
            Val(v) => {
                let key = v.to_text().unwrap_or_default();
                match map.get(&key) {
                    Some(mapped) => Val(mapped.clone()),
                    None => match on_unmapped {
                        Unmapped::Fail => Val(Value::Invalid { raw: key, reason: "no enum mapping for this value".into() }),
                        Unmapped::Keep => Val(v),
                        Unmapped::NullAndArchive => {
                            archived.push((source_label(input), v, "enum value has no destination equivalent".into()));
                            Val(Value::Null)
                        }
                    },
                }
            }
            Missing => Missing,
        },
        Expr::Coalesce { inputs } => {
            let mut saw_null = false;
            for i in inputs {
                match eval(i, r, ctx, archived) {
                    Val(Value::Null) => saw_null = true,
                    Val(v) => return Val(v),
                    Missing => {}
                }
            }
            if saw_null { Val(Value::Null) } else { Missing }
        }
        Expr::Default { input, value } => match eval(input, r, ctx, archived) {
            Val(v) if !v.is_null() => Val(v),
            _ => Val(value.clone()),
        },
        Expr::Trim { input } => map_str(eval(input, r, ctx, archived), |s| s.trim().to_string()),
        Expr::Lower { input } => map_str(eval(input, r, ctx, archived), |s| s.to_lowercase()),
        Expr::Upper { input } => map_str(eval(input, r, ctx, archived), |s| s.to_uppercase()),
        Expr::ParseDate { input, format, datetime } => match eval(input, r, ctx, archived) {
            Val(Value::Null) => Val(Value::Null),
            Val(v) => {
                let t = v.to_text().unwrap_or_default();
                let parsed = if *datetime {
                    chrono::DateTime::parse_from_str(&t, format)
                        .map(Value::DateTime)
                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&t, format).map(|n| Value::DateTime(n.and_utc().fixed_offset())))
                        .ok()
                } else {
                    chrono::NaiveDate::parse_from_str(&t, format).map(Value::Date).ok()
                };
                Val(parsed.unwrap_or(Value::Invalid { raw: t, reason: format!("does not match date format `{format}`") }))
            }
            Missing => Missing,
        },
        Expr::ScaleDecimal { input, pow } => match eval(input, r, ctx, archived) {
            Val(Value::Null) => Val(Value::Null),
            Val(v) => {
                let t = decimal_text(&v);
                Val(t
                    .as_deref()
                    .and_then(|t| scale_decimal(t, *pow))
                    .map(Value::Decimal)
                    .unwrap_or(Value::Invalid { raw: v.to_text().unwrap_or_default(), reason: "not a decimal number".into() }))
            }
            Missing => Missing,
        },
        Expr::UnitConvert { input, factor } => match eval(input, r, ctx, archived) {
            Val(Value::Null) => Val(Value::Null),
            Val(v) => {
                let t = decimal_text(&v);
                Val(t.as_deref().and_then(|t| mul_decimal(t, factor)).map(Value::Decimal).unwrap_or(Value::Invalid {
                    raw: v.to_text().unwrap_or_default(),
                    reason: "unit conversion failed (non-numeric or overflow)".into(),
                }))
            }
            Missing => Missing,
        },
        Expr::Ref { input, object } => match eval(input, r, ctx, archived) {
            Val(Value::Null) => Val(Value::Null),
            Val(Value::String(s)) if s.is_empty() => Val(Value::Null),
            Val(v) => {
                let key = v.to_text().unwrap_or_default();
                match ctx.refs.resolve(object, &key) {
                    Some(dest) => Val(match dest.parse::<i64>() {
                        Ok(i) if !dest.starts_with('0') || dest == "0" => Value::Int(i),
                        _ => Value::String(dest),
                    }),
                    None => Val(Value::Invalid { raw: key, reason: format!("unresolved reference to `{object}`") }),
                }
            }
            Missing => Missing,
        },
        Expr::Extension { input, name } => match eval(input, r, ctx, archived) {
            Val(v) => match ctx.extensions {
                Some(x) => Val(x.call(name, &v).unwrap_or_else(|e| Value::Invalid {
                    raw: v.to_text().unwrap_or_default(),
                    reason: format!("extension `{name}` failed: {}", e.message),
                })),
                None => Val(Value::Invalid { raw: String::new(), reason: format!("extension `{name}` is not loaded") }),
            },
            Missing => Missing,
        },
        Expr::Object { fields } => {
            let mut m = BTreeMap::new();
            for (k, e) in fields {
                if let Val(v) = eval(e, r, ctx, archived) {
                    m.insert(k.clone(), v);
                }
            }
            Val(Value::Object(m))
        }
    }
}

fn source_label(e: &Expr) -> String {
    let mut p = vec![];
    e.source_paths(&mut p);
    p.join("+")
}

fn map_str(e: Eval, f: impl Fn(&str) -> String) -> Eval {
    match e {
        Eval::Val(Value::String(s)) => Eval::Val(Value::String(f(&s))),
        other => other,
    }
}

fn decimal_text(v: &Value) -> Option<String> {
    match v {
        Value::Int(i) => Some(i.to_string()),
        Value::Decimal(d) => Some(d.clone()),
        Value::Float(f) => Some(format!("{f}")),
        Value::String(s) => {
            let t = s.trim();
            (t.parse::<i64>().is_ok() || is_decimal_text(t)).then(|| t.to_string())
        }
        _ => None,
    }
}

/// Exact decimal multiplication using i128 mantissas.
pub fn mul_decimal(a: &str, b: &str) -> Option<String> {
    fn split(s: &str) -> Option<(i128, u32)> {
        let s = normalize_decimal(s);
        let (neg, body) = match s.strip_prefix('-') {
            Some(b) => (true, b.to_string()),
            None => (false, s),
        };
        let (i, f) = body.split_once('.').unwrap_or((&body, ""));
        let m: i128 = format!("{i}{f}").parse().ok()?;
        Some((if neg { -m } else { m }, f.len() as u32))
    }
    let (ma, sa) = split(a)?;
    let (mb, sb) = split(b)?;
    let m = ma.checked_mul(mb)?;
    let scale = sa + sb;
    let neg = m < 0;
    let digits = m.unsigned_abs().to_string();
    let s = if scale == 0 {
        digits
    } else if digits.len() as u32 > scale {
        let p = digits.len() - scale as usize;
        format!("{}.{}", &digits[..p], &digits[p..])
    } else {
        format!("0.{}{}", "0".repeat(scale as usize - digits.len()), digits)
    };
    // trim trailing zeros introduced by the factor's scale, keep at least the input scale
    let s = if s.contains('.') {
        let keep = sa as usize;
        let (i, f) = s.split_once('.').expect("dot");
        let mut f = f.to_string();
        while f.len() > keep && f.ends_with('0') {
            f.pop();
        }
        if f.is_empty() { i.to_string() } else { format!("{i}.{f}") }
    } else {
        s
    };
    Some(if neg { format!("-{s}") } else { s })
}

/// Convert a value to a field type; failures become `Value::Invalid` (never silently dropped).
pub fn cast_value(v: &Value, to: FieldType) -> Value {
    if v.field_type() == to || to == FieldType::Unknown || v.is_null() || v.is_problem() {
        return v.clone();
    }
    let invalid = |reason: &str| Value::Invalid { raw: v.to_text().unwrap_or_default(), reason: reason.to_string() };
    match (v, to) {
        (_, FieldType::String) => Value::String(v.to_text().unwrap_or_default()),
        (Value::Float(f), FieldType::Int) if f.fract() == 0.0 && f.abs() < 9.0e15 => Value::Int(*f as i64),
        (Value::Decimal(d), FieldType::Int) => {
            let (i, frac) = d.split_once('.').unwrap_or((d.as_str(), ""));
            if frac.chars().all(|c| c == '0') {
                i.parse().map(Value::Int).unwrap_or_else(|_| invalid("out of range"))
            } else {
                invalid("has a fractional part")
            }
        }
        (Value::Bool(b), FieldType::Int) => Value::Int(*b as i64),
        (Value::Int(i), FieldType::Float) => Value::Float(*i as f64),
        (Value::Decimal(d), FieldType::Float) => d.parse().map(Value::Float).unwrap_or_else(|_| invalid("not a number")),
        (Value::Int(i), FieldType::Decimal) => Value::Decimal(i.to_string()),
        (Value::Float(f), FieldType::Decimal) => Value::Decimal(format!("{f}")),
        (Value::Int(i), FieldType::Bool) if *i == 0 || *i == 1 => Value::Bool(*i == 1),
        (Value::DateTime(dt), FieldType::Date) => Value::Date(dt.date_naive()),
        (Value::Date(d), FieldType::DateTime) => Value::DateTime(d.and_hms_opt(0, 0, 0).expect("midnight").and_utc().fixed_offset()),
        (Value::String(s), t) => Value::parse_as(s, t),
        (Value::Array(_) | Value::Object(_), _) => invalid("structured value cannot be converted"),
        _ => invalid("incompatible types"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_refs(_: &str, _: &str) -> Option<String> {
        None
    }

    fn om(fields: Vec<FieldMapping>) -> ObjectMapping {
        ObjectMapping {
            source_object: "s".into(),
            target_object: "t".into(),
            target_key_field: None,
            fields,
            unmapped_fields: UnmappedFieldPolicy::Archive,
            skip_fields: vec![],
            write_mode: WriteMode::Upsert,
        }
    }

    #[test]
    fn transforms_and_archives() {
        let rec = Record::new("s", "1")
            .with("first", Value::String("Ann".into()))
            .with("last", Value::String("Lee".into()))
            .with("status", Value::String("A".into()))
            .with("amount", Value::Decimal("12.34".into()))
            .with("secret_sauce", Value::String("x".into()))
            .with("addr", Value::Object([("city".to_string(), Value::String("Oslo".into()))].into_iter().collect()));
        let m = om(vec![
            FieldMapping::accepted("name", Expr::Concat { parts: vec![Expr::field("first"), Expr::field("last")], separator: " ".into() }),
            FieldMapping::accepted(
                "state",
                Expr::EnumMap {
                    input: Box::new(Expr::field("status")),
                    map: [("A".to_string(), Value::String("active".into()))].into_iter().collect(),
                    on_unmapped: Unmapped::Fail,
                },
            ),
            FieldMapping::accepted("cents", Expr::ScaleDecimal { input: Box::new(Expr::field("amount")), pow: 2 }),
            FieldMapping::accepted("city", Expr::field("addr.city")),
            FieldMapping::accepted("missing", Expr::field("nope")),
        ]);
        let consumed = consumed_fields(&m);
        let ctx = EvalContext { refs: &no_refs, extensions: None };
        let t = transform_record(&m, None, &consumed, &rec, &ctx, false);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.fields["name"], Value::String("Ann Lee".into()));
        assert_eq!(t.fields["state"], Value::String("active".into()));
        assert_eq!(t.fields["cents"], Value::Decimal("1234".into()));
        assert_eq!(t.fields["city"], Value::String("Oslo".into()));
        assert!(!t.fields.contains_key("missing"), "missing stays missing");
        assert!(t.transformed);
        assert_eq!(t.archived.len(), 1);
        assert_eq!(t.archived[0].0, "secret_sauce");
    }

    #[test]
    fn unmapped_enum_fails_record_and_refs_resolve() {
        let rec = Record::new("s", "1").with("status", Value::String("Z".into())).with("org", Value::Int(7));
        let m = om(vec![
            FieldMapping::accepted(
                "state",
                Expr::EnumMap { input: Box::new(Expr::field("status")), map: BTreeMap::new(), on_unmapped: Unmapped::Fail },
            ),
            FieldMapping::accepted("org_id", Expr::Ref { input: Box::new(Expr::field("org")), object: "orgs".into() }),
        ]);
        let refs = |o: &str, id: &str| (o == "orgs" && id == "7").then(|| "1001".to_string());
        let ctx = EvalContext { refs: &refs, extensions: None };
        let t = transform_record(&m, None, &consumed_fields(&m), &rec, &ctx, false);
        assert_eq!(t.errors.len(), 1);
        assert_eq!(t.fields["org_id"], Value::Int(1001));
    }

    #[test]
    fn required_fields_enforced() {
        let mut def = ma_connector_free_def();
        def.fields[0].required = true;
        let m = om(vec![FieldMapping::accepted("name", Expr::field("n"))]);
        let ctx = EvalContext { refs: &no_refs, extensions: None };
        let t = transform_record(&m, Some(&def), &consumed_fields(&m), &Record::new("s", "1"), &ctx, false);
        assert_eq!(t.errors.len(), 1);
    }

    fn ma_connector_free_def() -> ObjectDef {
        ObjectDef {
            name: "t".into(),
            fields: vec![FieldDef::new("name", FieldType::String)],
            primary_key: vec![],
            unique_keys: vec![],
            estimated_count: None,
            readable: true,
            writable: true,
        }
    }

    #[test]
    fn casts() {
        assert_eq!(cast_value(&Value::String("42".into()), FieldType::Int), Value::Int(42));
        assert!(matches!(cast_value(&Value::String("4x".into()), FieldType::Int), Value::Invalid { .. }));
        assert_eq!(cast_value(&Value::Decimal("3.00".into()), FieldType::Int), Value::Int(3));
        assert!(matches!(cast_value(&Value::Decimal("3.5".into()), FieldType::Int), Value::Invalid { .. }));
        assert_eq!(mul_decimal("2.5", "1.609344").unwrap(), "4.02336");
        assert_eq!(mul_decimal("-10", "0.5").unwrap(), "-5");
    }

    proptest::proptest! {
        #[test]
        fn cast_to_string_never_loses_int(i in proptest::num::i64::ANY) {
            let s = cast_value(&Value::Int(i), FieldType::String);
            proptest::prop_assert_eq!(cast_value(&s, FieldType::Int), Value::Int(i));
        }
    }
}
