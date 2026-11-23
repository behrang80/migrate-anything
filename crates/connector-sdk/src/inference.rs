//! Schema inference from sampled records (used by file and API connectors).

use std::collections::BTreeMap;

use ma_types::{FieldDef, FieldType, ObjectDef, Record, Value};

#[derive(Default)]
struct FieldStats {
    types: BTreeMap<&'static str, u64>,
    nulls: u64,
    present: u64,
    first_type: Option<FieldType>,
}

fn type_key(t: FieldType) -> &'static str {
    match t {
        FieldType::String => "string",
        FieldType::Int => "int",
        FieldType::Float => "float",
        FieldType::Decimal => "decimal",
        FieldType::Bool => "bool",
        FieldType::Date => "date",
        FieldType::DateTime => "datetime",
        FieldType::Bytes => "bytes",
        FieldType::Array => "array",
        FieldType::Object => "object",
        FieldType::Unknown => "unknown",
    }
}

fn from_key(k: &str) -> FieldType {
    match k {
        "string" => FieldType::String,
        "int" => FieldType::Int,
        "float" => FieldType::Float,
        "decimal" => FieldType::Decimal,
        "bool" => FieldType::Bool,
        "date" => FieldType::Date,
        "datetime" => FieldType::DateTime,
        "bytes" => FieldType::Bytes,
        "array" => FieldType::Array,
        "object" => FieldType::Object,
        _ => FieldType::Unknown,
    }
}

/// Infer an object definition from a sample. Mixed numeric types widen (int → decimal/float);
/// any other mix widens to string. Fields absent from some records are marked not required.
pub fn infer_object(name: &str, sample: &[Record], id_field: Option<&str>) -> ObjectDef {
    let mut stats: BTreeMap<String, FieldStats> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    for r in sample {
        for (k, v) in &r.fields {
            let s = stats.entry(k.clone()).or_insert_with(|| {
                order.push(k.clone());
                FieldStats::default()
            });
            s.present += 1;
            match v {
                Value::Null => s.nulls += 1,
                Value::String(x) if x.is_empty() => s.nulls += 1,
                other => {
                    let t = other.field_type();
                    s.first_type.get_or_insert(t);
                    *s.types.entry(type_key(t)).or_default() += 1;
                }
            }
        }
    }
    let n = sample.len() as u64;
    let fields = order
        .iter()
        .map(|k| {
            let s = &stats[k];
            let kinds: Vec<FieldType> = s.types.keys().map(|k| from_key(k)).collect();
            let field_type = widen(&kinds);
            let mut f = FieldDef::new(k.clone(), field_type);
            f.nullable = s.nulls > 0 || s.present < n;
            f.required = s.present == n && s.nulls == 0 && n > 0;
            f
        })
        .collect();
    let pk = match id_field {
        Some(f) => vec![f.to_string()],
        None => order.iter().find(|k| k.as_str() == "id").cloned().into_iter().collect(),
    };
    ObjectDef {
        name: name.to_string(),
        fields,
        primary_key: pk,
        unique_keys: vec![],
        estimated_count: None,
        readable: true,
        writable: true,
    }
}

fn widen(kinds: &[FieldType]) -> FieldType {
    use FieldType::*;
    match kinds {
        [] => Unknown,
        [one] => *one,
        many => {
            if many.iter().all(|k| matches!(k, Int | Float | Decimal)) {
                if many.contains(&Float) { Float } else { Decimal }
            } else if many.iter().all(|k| matches!(k, Date | DateTime)) {
                DateTime
            } else {
                String
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_and_required() {
        let a = Record::new("o", "1").with("id", Value::Int(1)).with("amt", Value::Int(3)).with("x", Value::Null);
        let b = Record::new("o", "2").with("id", Value::Int(2)).with("amt", Value::Decimal("1.5".into()));
        let def = infer_object("o", &[a, b], None);
        assert_eq!(def.field("amt").unwrap().field_type, FieldType::Decimal);
        assert!(def.field("id").unwrap().required);
        assert!(!def.field("x").unwrap().required);
        assert_eq!(def.primary_key, vec!["id".to_string()]);
    }
}
