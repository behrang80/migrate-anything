//! Redaction of secrets from text destined for logs, errors, reports and MCP responses.

/// Remove URL userinfo passwords (`scheme://user:pass@host` → `scheme://user:***@host`).
pub fn redact_url(s: &str) -> String {
    match url::Url::parse(s) {
        Ok(mut u) if u.password().is_some() => {
            let _ = u.set_password(Some("***"));
            u.to_string()
        }
        _ => s.to_string(),
    }
}

/// Redact known secret values and common credential patterns.
pub fn redact(text: &str, known_secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in known_secrets {
        if s.len() >= 4 {
            out = out.replace(s, "***");
        }
    }
    // password=..., token=..., api_key=... up to a delimiter
    for key in ["password=", "passwd=", "token=", "api_key=", "apikey=", "secret="] {
        let mut search_from = 0;
        while let Some(pos) = out[search_from..].to_ascii_lowercase().find(key) {
            let start = search_from + pos + key.len();
            let end = out[start..].find(['&', ' ', ';', '"', '\'', '\n']).map(|e| start + e).unwrap_or(out.len());
            if end > start {
                out.replace_range(start..end, "***");
            }
            search_from = start + 3.min(out.len() - start);
        }
    }
    // Bearer tokens
    if let Some(pos) = out.find("Bearer ") {
        let start = pos + 7;
        let end = out[start..].find(|c: char| c.is_whitespace() || c == '"').map(|e| start + e).unwrap_or(out.len());
        out.replace_range(start..end, "***");
    }
    // URL passwords
    out.split(' ').map(|w| if w.contains("://") { redact_url(w) } else { w.to_string() }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_patterns() {
        let r = redact("connect postgres://u:hunter22@h/db failed password=abc123&x=1 Bearer tok.en", &["topsecret"]);
        assert!(!r.contains("hunter22"));
        assert!(!r.contains("abc123"));
        assert!(!r.contains("tok.en"));
        assert_eq!(redact("value topsecret here", &["topsecret"]), "value *** here");
    }
}
