//! SQL text helpers, value encoding and error classification.

use base64::Engine as _;
use ma_types::{ErrorClass, MaError, MaResult, Value};

use crate::catalog::{ColMeta, TableMeta};

/// Quote an identifier for interpolation into SQL (doubles embedded `"`).
pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub(crate) fn table_ref(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

/// Map a sqlx error to a classified [`MaError`]. Messages never contain connection secrets:
/// sqlx error displays do not include connect options.
pub(crate) fn pg_err(context: &str, e: sqlx::Error) -> MaError {
    let class = match &e {
        sqlx::Error::Database(db) => {
            let code = db.code().map(|c| c.into_owned()).unwrap_or_default();
            classify_sqlstate(&code)
        }
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::Protocol(_)
        | sqlx::Error::BeginFailed => ErrorClass::Transient,
        sqlx::Error::Configuration(_) => ErrorClass::Config,
        sqlx::Error::Encode(_) | sqlx::Error::Decode(_) | sqlx::Error::ColumnDecode { .. } => ErrorClass::Validation,
        _ => ErrorClass::Internal,
    };
    MaError::new(class, format!("postgres {context}: {e}"))
}

pub(crate) fn classify_sqlstate(code: &str) -> ErrorClass {
    match code {
        "28000" | "28P01" | "42501" => ErrorClass::Auth,
        "40001" | "40P01" | "55P03" | "57014" => ErrorClass::Transient,
        "42P01" | "42703" | "3F000" | "3D000" => ErrorClass::Config,
        c if c.starts_with("08") || c.starts_with("53") || c.starts_with("57P") => ErrorClass::Transient,
        c if c.starts_with("22") || c.starts_with("23") || c == "21000" => ErrorClass::Validation,
        _ => ErrorClass::Internal,
    }
}

/// Expression reading a column as lossless text.
pub(crate) fn read_expr(c: &ColMeta) -> String {
    let q = quote_ident(&c.name);
    if c.typname == "numeric" {
        format!("{q}::text")
    } else if c.is_bytea() {
        format!("replace(encode({q}, 'base64'), chr(10), '')")
    } else if c.is_json() || c.is_array() {
        format!("to_json({q})::text")
    } else {
        format!("to_json({q}) #>> '{{}}'")
    }
}

pub(crate) fn select_list(t: &TableMeta) -> String {
    t.columns.iter().map(read_expr).collect::<Vec<_>>().join(", ")
}

/// Placeholder expression with an explicit cast to the column type.
pub(crate) fn param_expr(c: &ColMeta, n: usize) -> String {
    if c.is_bytea() { format!("decode(${n}::text, 'base64')") } else { format!("${n}::{}", c.cast_type) }
}

/// Encode a value as the text parameter bound for column `c`.
pub(crate) fn to_param(v: &Value, c: &ColMeta) -> MaResult<Option<String>> {
    if let Value::Invalid { raw, reason } = v {
        return Err(MaError::validation(format!("field `{}`: invalid value `{raw}`: {reason}", c.name)));
    }
    if let Value::Unsupported { type_name, .. } = v {
        return Err(MaError::validation(format!("field `{}`: unsupported value type `{type_name}`", c.name)));
    }
    if v.is_null() {
        return Ok(None);
    }
    if c.is_json() {
        return Ok(Some(v.to_plain_json().to_string()));
    }
    if c.is_array() {
        return Ok(Some(match v {
            Value::Array(items) => pg_array_literal(items),
            other => other.to_text().unwrap_or_default(),
        }));
    }
    if c.is_bytea() {
        return Ok(Some(match v {
            Value::Bytes(b) => b.clone(),
            other => base64::engine::general_purpose::STANDARD.encode(other.to_text().unwrap_or_default()),
        }));
    }
    if matches!(v, Value::Array(_) | Value::Object(_)) && !matches!(c.field_type, ma_types::FieldType::String) {
        return Err(MaError::validation(format!("field `{}`: cannot write a structured value into a `{}` column", c.name, c.native)));
    }
    Ok(v.to_text())
}

/// Build a PostgreSQL array literal (`{"a","b",NULL}`), nesting for multi-dimensional arrays.
pub(crate) fn pg_array_literal(items: &[Value]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|v| match v {
            Value::Null => "NULL".to_string(),
            Value::Array(inner) => pg_array_literal(inner),
            Value::Object(_) => quote_elem(&v.to_plain_json().to_string()),
            other => quote_elem(&other.to_text().unwrap_or_default()),
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

fn quote_elem(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(pg_array_literal(&[Value::String("x\"y".into()), Value::Null, Value::Int(3)]), "{\"x\\\"y\",NULL,\"3\"}");
        assert_eq!(classify_sqlstate("23502"), ErrorClass::Validation);
        assert_eq!(classify_sqlstate("28P01"), ErrorClass::Auth);
        assert_eq!(classify_sqlstate("08006"), ErrorClass::Transient);
    }
}
