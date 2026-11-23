//! Canonical JSON hashing (keys sorted recursively, no whitespace).

use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn canonical_json<T: Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serializable");
    let mut out = String::new();
    write_canonical(&v, &mut out);
    out
}

pub fn canonical_sha256<T: Serialize>(value: &T) -> String {
    sha256_hex(canonical_json(value).as_bytes())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_canonical(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value as J;
    match v {
        J::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("string"));
                out.push(':');
                write_canonical(&map[*k], out);
            }
            out.push('}');
        }
        J::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_order_does_not_matter() {
        let a: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":{"y":2,"x":[1,2]}}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"a":{"x":[1,2],"y":2},"b":1}"#).unwrap();
        assert_eq!(canonical_sha256(&a), canonical_sha256(&b));
    }
}
