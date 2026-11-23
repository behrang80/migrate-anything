//! Strict static validation of declarative connector specs (generated or hand-written).

use std::collections::BTreeSet;

use ma_types::*;

/// Parse a spec from JSON with unknown fields rejected.
pub fn parse_spec(json: &str) -> MaResult<DeclarativeConnectorSpec> {
    serde_json::from_str(json).map_err(|e| MaError::validation(format!("invalid connector spec: {e}")))
}

fn safe_path(p: &str, allow_id: bool) -> Result<(), String> {
    if !p.starts_with('/') {
        return Err(format!("path `{p}` must start with `/`"));
    }
    if p.len() > 512 {
        return Err("path too long".into());
    }
    for bad in ["://", "..", "?", "#", "@", "\\", "%2e", "%2E", "//"] {
        if p.contains(bad) {
            return Err(format!("path `{p}` contains forbidden sequence `{bad}`"));
        }
    }
    if p.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("path `{p}` contains whitespace/control characters"));
    }
    let stripped = p.replace("{id}", "");
    if stripped.contains('{') || stripped.contains('}') {
        return Err(format!("path `{p}` uses a placeholder other than {{id}}"));
    }
    if !allow_id && p.contains("{id}") {
        return Err(format!("path `{p}` may not contain {{id}}"));
    }
    Ok(())
}

fn ident_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Returns every problem found; an empty list means the spec is acceptable.
pub fn validate_spec(spec: &DeclarativeConnectorSpec) -> Vec<String> {
    let mut errs = vec![];
    if spec.spec_version != CONNECTOR_SPEC_VERSION {
        errs.push(format!("unsupported spec_version {}", spec.spec_version));
    }
    if !(3..=64).contains(&spec.connector_id.len())
        || !spec.connector_id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errs.push("connector_id must be 3-64 chars of [a-z0-9-]".into());
    }
    let parts: Vec<&str> = spec.version.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        errs.push("version must be MAJOR.MINOR.PATCH".into());
    }
    if spec.title.len() > 200 {
        errs.push("title too long".into());
    }
    if !spec.base_path.is_empty()
        && let Err(e) = safe_path(&spec.base_path, false)
    {
        errs.push(format!("base_path: {e}"));
    }
    if let AuthSpec::ApiKeyHeader { header } = &spec.authentication {
        let h = header.to_ascii_lowercase();
        if header.is_empty() || !header.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            errs.push("auth header name is invalid".into());
        }
        if ["host", "cookie", "content-length", "transfer-encoding", "connection", "content-type"].contains(&h.as_str()) {
            errs.push(format!("auth header `{header}` is not allowed"));
        }
    }
    if let Some(rl) = &spec.rate_limit
        && rl.max_requests_per_second > 10_000
    {
        errs.push("rate limit too high".into());
    }
    if spec.resources.is_empty() {
        errs.push("spec defines no resources".into());
    }
    if spec.resources.len() > 500 {
        errs.push("too many resources".into());
    }
    let mut names = BTreeSet::new();
    for r in &spec.resources {
        let ctx = format!("resource `{}`", r.name);
        if !ident_ok(&r.name, 64) {
            errs.push(format!("{ctx}: name must be [a-z0-9_-]{{1,64}}"));
        }
        if !names.insert(r.name.clone()) {
            errs.push(format!("{ctx}: duplicate resource name"));
        }
        if r.identity_field.is_empty() || r.identity_field.len() > 128 {
            errs.push(format!("{ctx}: identity_field is required"));
        }
        if let Some(l) = &r.list {
            if let Err(e) = safe_path(&l.path, false) {
                errs.push(format!("{ctx}.list: {e}"));
            }
            let ps = match &l.pagination {
                Pagination::None => None,
                Pagination::Cursor { page_size, cursor_param, .. } => {
                    if cursor_param.is_empty() {
                        errs.push(format!("{ctx}: cursor_param empty"));
                    }
                    Some(*page_size)
                }
                Pagination::Offset { page_size, .. } | Pagination::Page { page_size, .. } => Some(*page_size),
            };
            if let Some(ps) = ps
                && !(1..=1000).contains(&ps)
            {
                errs.push(format!("{ctx}: page_size must be 1..=1000"));
            }
        }
        for (label, op, allowed, allow_id) in [
            ("create", &r.create, &[HttpMethod::Post, HttpMethod::Put][..], false),
            ("update", &r.update, &[HttpMethod::Put, HttpMethod::Patch][..], true),
        ] {
            if let Some(op) = op {
                if !allowed.contains(&op.method) {
                    errs.push(format!("{ctx}.{label}: method {:?} not allowed", op.method));
                }
                if let Err(e) = safe_path(&op.path, allow_id) {
                    errs.push(format!("{ctx}.{label}: {e}"));
                }
                if label == "update" && !op.path.contains("{id}") {
                    errs.push(format!("{ctx}.update: path must contain {{id}}"));
                }
            }
        }
        if let Some(g) = &r.get
            && let Err(e) = safe_path(&g.path, true)
        {
            errs.push(format!("{ctx}.get: {e}"));
        }
        if let Some(l) = &r.lookup {
            if let Err(e) = safe_path(&l.path, false) {
                errs.push(format!("{ctx}.lookup: {e}"));
            }
            if l.query_param.is_empty() || l.key_field.is_empty() {
                errs.push(format!("{ctx}.lookup: query_param and key_field are required"));
            }
        }
        if r.list.is_none() && r.create.is_none() {
            errs.push(format!("{ctx}: neither readable nor writable"));
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DeclarativeConnectorSpec {
        DeclarativeConnectorSpec {
            spec_version: 1,
            connector_id: "demo-api".into(),
            version: "1.0.0".into(),
            protocol: SpecProtocol::Rest,
            title: "Demo".into(),
            api_version: None,
            base_path: "/api".into(),
            authentication: AuthSpec::Bearer,
            rate_limit: None,
            resources: vec![ResourceSpec {
                name: "items".into(),
                identity_field: "id".into(),
                list: Some(ListOperation { path: "/items".into(), records_path: String::new(), pagination: Pagination::None }),
                create: None,
                update: None,
                get: None,
                lookup: None,
                fields: vec![],
                relationships: vec![],
            }],
            provenance: SpecProvenance {
                generated_by: "test".into(),
                generated_at: "now".into(),
                source_document_sha256: None,
                notes: vec![],
            },
        }
    }

    #[test]
    fn accepts_valid_spec() {
        assert!(validate_spec(&base()).is_empty());
    }

    #[test]
    fn rejects_malicious_specs() {
        let mut s = base();
        s.resources[0].list.as_mut().unwrap().path = "http://169.254.169.254/latest/meta-data".into();
        assert!(!validate_spec(&s).is_empty());
        let mut s = base();
        s.resources[0].list.as_mut().unwrap().path = "/items/../../admin".into();
        assert!(!validate_spec(&s).is_empty());
        let mut s = base();
        s.base_path = "@evil.example.com".into();
        assert!(!validate_spec(&s).is_empty());
        let mut s = base();
        s.authentication = AuthSpec::ApiKeyHeader { header: "Host".into() };
        assert!(!validate_spec(&s).is_empty());
        let mut s = base();
        s.resources[0].create =
            Some(WriteOperation { method: HttpMethod::Delete, path: "/items".into(), response_id_path: String::new(), idempotent: false });
        assert!(!validate_spec(&s).is_empty());
        // unknown fields (e.g. an embedded credential or host override) fail to parse
        let mut j = serde_json::to_value(base()).unwrap();
        j["api_key"] = serde_json::json!("sk-live-123");
        assert!(parse_spec(&j.to_string()).is_err());
        let mut j = serde_json::to_value(base()).unwrap();
        j["base_url"] = serde_json::json!("http://evil.example.com");
        assert!(parse_spec(&j.to_string()).is_err());
    }
}
