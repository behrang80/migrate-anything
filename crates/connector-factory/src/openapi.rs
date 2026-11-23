//! OpenAPI 3.x → capability manifest + declarative connector spec.

use ma_types::*;
use serde_json::Value as J;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GenerationResult {
    pub spec: DeclarativeConnectorSpec,
    pub manifest: CapabilityManifest,
    /// Human-readable notes about inferences and things that were not supported.
    pub notes: Vec<String>,
    /// Text in the document that appears to address AI agents. Reported, never obeyed.
    pub suspicious_text: Vec<String>,
    pub validation_errors: Vec<String>,
}

/// Parse an OpenAPI document (JSON or YAML) into a generic JSON tree. Size-limited.
pub fn parse_document(bytes: &[u8]) -> MaResult<J> {
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(MaError::validation("API document larger than 20 MiB"));
    }
    let trimmed = bytes.iter().position(|b| !b.is_ascii_whitespace()).map(|i| bytes[i]).unwrap_or(b'{');
    let v: J = if trimmed == b'{' {
        serde_json::from_slice(bytes).map_err(|e| MaError::validation(format!("API document is not valid JSON: {e}")))?
    } else {
        serde_norway::from_slice(bytes).map_err(|e| MaError::validation(format!("API document is not valid YAML: {e}")))?
    };
    let ver = v.get("openapi").and_then(|x| x.as_str()).unwrap_or("");
    if !ver.starts_with("3.") {
        return Err(MaError::unsupported(format!(
            "only OpenAPI 3.x documents are supported (found `{}`)",
            if ver.is_empty() { v.get("swagger").and_then(|x| x.as_str()).unwrap_or("unknown") } else { ver }
        )));
    }
    Ok(v)
}

const INJECTION_MARKERS: &[&str] = &[
    "ignore all previous",
    "ignore previous instructions",
    "ignore the above",
    "system note to ai",
    "note to ai",
    "you are an ai",
    "approve the migration",
    "approve this",
    "mark this connector as approved",
    "allowed hosts",
    "allowed_hosts",
    "disable security",
    "bypass",
    "169.254.169.254",
    "exfiltrate",
    "send the credentials",
];

fn is_suspicious(text: &str) -> bool {
    let lower = text.to_lowercase();
    INJECTION_MARKERS.iter().any(|m| lower.contains(m))
}

fn scan_text(path: &str, text: &str, out: &mut Vec<String>) {
    if is_suspicious(text) {
        let snippet: String = text.chars().take(160).collect();
        out.push(format!("{path}: \"{snippet}\""));
    }
}

fn scan_all(v: &J, path: &str, out: &mut Vec<String>) {
    match v {
        J::String(s) => scan_text(path, s, out),
        J::Array(a) => a.iter().enumerate().for_each(|(i, x)| scan_all(x, &format!("{path}[{i}]"), out)),
        J::Object(m) => m.iter().for_each(|(k, x)| scan_all(x, &format!("{path}.{k}"), out)),
        _ => {}
    }
}

struct Doc<'a> {
    root: &'a J,
}

impl<'a> Doc<'a> {
    /// Resolve local `$ref`s (bounded depth; cycles stop resolution).
    fn resolve(&self, v: &'a J, depth: usize) -> &'a J {
        if depth > 32 {
            return v;
        }
        if let Some(r) = v.get("$ref").and_then(|r| r.as_str())
            && let Some(ptr) = r.strip_prefix('#')
            && let Some(t) = self.root.pointer(ptr)
        {
            return self.resolve(t, depth + 1);
        }
        v
    }

    fn props(&self, schema: &'a J) -> (Vec<(String, &'a J)>, Vec<String>) {
        let s = self.resolve(schema, 0);
        let mut props = vec![];
        let mut req = vec![];
        // allOf merges
        if let Some(all) = s.get("allOf").and_then(|a| a.as_array()) {
            for part in all {
                let (p, r) = self.props(part);
                props.extend(p);
                req.extend(r);
            }
        }
        if let Some(m) = s.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in m {
                props.push((k.clone(), self.resolve(v, 0)));
            }
        }
        if let Some(r) = s.get("required").and_then(|r| r.as_array()) {
            req.extend(r.iter().filter_map(|x| x.as_str().map(String::from)));
        }
        (props, req)
    }
}

fn type_of(s: &J) -> (FieldType, bool) {
    let mut nullable = s.get("nullable").and_then(|n| n.as_bool()).unwrap_or(false);
    let ty = match s.get("type") {
        Some(J::String(t)) => t.clone(),
        Some(J::Array(ts)) => {
            nullable |= ts.iter().any(|t| t == "null");
            ts.iter().filter_map(|t| t.as_str()).find(|t| *t != "null").unwrap_or("string").to_string()
        }
        _ => {
            if s.get("properties").is_some() {
                "object".into()
            } else {
                "unknown".into()
            }
        }
    };
    let fmt = s.get("format").and_then(|f| f.as_str()).unwrap_or("");
    let ft = match (ty.as_str(), fmt) {
        ("string", "date") => FieldType::Date,
        ("string", "date-time") => FieldType::DateTime,
        ("string", "byte" | "binary") => FieldType::Bytes,
        ("string", _) => FieldType::String,
        ("integer", _) => FieldType::Int,
        ("number", "float" | "double") => FieldType::Float,
        ("number", _) => FieldType::Decimal,
        ("boolean", _) => FieldType::Bool,
        ("array", _) => FieldType::Array,
        ("object", _) => FieldType::Object,
        _ => FieldType::Unknown,
    };
    (ft, nullable)
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if s.is_empty() { "resource".into() } else { s.chars().take(64).collect() }
}

const CURSOR_PARAMS: &[&str] =
    &["cursor", "after", "page_token", "pagetoken", "next", "starting_after", "continuation", "next_token", "continuation_token"];
const LIMIT_PARAMS: &[&str] = &["limit", "page_size", "pagesize", "per_page", "size", "count", "max_results", "maxresults", "top"];
const OFFSET_PARAMS: &[&str] = &["offset", "skip", "start"];
const PAGE_PARAMS: &[&str] = &["page", "page_number", "pagenumber"];
const NEXT_FIELDS: &[&str] =
    &["next_cursor", "nextcursor", "next", "cursor", "next_page_token", "nextpagetoken", "continuation_token", "next_token", "after"];

/// Generate a connector for the API described by `doc`.
pub fn generate_from_openapi(doc_json: &J, connector_id: &str, source_sha256: &str) -> MaResult<GenerationResult> {
    let doc = Doc { root: doc_json };
    let mut notes = vec![];
    let mut suspicious = vec![];
    scan_all(doc_json, "$", &mut suspicious);

    let title = doc_json.pointer("/info/title").and_then(|t| t.as_str()).unwrap_or("Unknown API").chars().take(200).collect::<String>();
    let title = if is_suspicious(&title) { "Untitled API (title withheld)".to_string() } else { title };
    let api_version = doc_json.pointer("/info/version").and_then(|t| t.as_str()).map(String::from);

    // base path from the first server (host is ignored: it comes from the operator's connection)
    let mut base_path = String::new();
    if let Some(url) = doc_json.pointer("/servers/0/url").and_then(|u| u.as_str()) {
        let path = if let Some(i) = url.find("://") {
            let rest = &url[i + 3..];
            rest.find('/').map(|j| rest[j..].to_string()).unwrap_or_default()
        } else {
            url.to_string()
        };
        let path = path.trim_end_matches('/').to_string();
        if path.starts_with('/') && !path.contains('{') {
            base_path = path;
        } else if !path.is_empty() {
            notes.push(format!("server URL `{url}` has a templated/relative path that was not used"));
        }
        if url.contains("://") {
            notes.push(format!(
                "documented server `{url}` is informational only; the connection's base_url and allowlist decide where requests go"
            ));
        }
    }

    // authentication
    let schemes = doc_json.pointer("/components/securitySchemes").and_then(|s| s.as_object());
    let required_schemes: Vec<String> = doc_json
        .get("security")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().filter_map(|o| o.as_object()).flat_map(|o| o.keys().cloned()).collect())
        .unwrap_or_default();
    let mut authentication = AuthSpec::None;
    let mut auth_ev = Evidence::Documented;
    let mut auth_detail = "no authentication documented".to_string();
    let candidates: Vec<String> =
        if required_schemes.is_empty() { schemes.map(|s| s.keys().cloned().collect()).unwrap_or_default() } else { required_schemes };
    for name in candidates {
        let Some(s) = schemes.and_then(|s| s.get(&name)) else { continue };
        let s = doc.resolve(s, 0);
        let ty = s.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "apiKey" if s.get("in").and_then(|i| i.as_str()) == Some("header") => {
                let h = s.get("name").and_then(|n| n.as_str()).unwrap_or("X-Api-Key").to_string();
                auth_detail = format!("API key in header `{h}`");
                authentication = AuthSpec::ApiKeyHeader { header: h };
                break;
            }
            "apiKey" => {
                auth_ev = Evidence::Unsupported;
                auth_detail = "API key in query/cookie is not supported (secrets must not be placed in URLs)".into();
            }
            "http" if s.get("scheme").and_then(|x| x.as_str()).map(|x| x.eq_ignore_ascii_case("bearer")) == Some(true) => {
                authentication = AuthSpec::Bearer;
                auth_detail = "HTTP bearer token".into();
                break;
            }
            "http" if s.get("scheme").and_then(|x| x.as_str()).map(|x| x.eq_ignore_ascii_case("basic")) == Some(true) => {
                authentication = AuthSpec::Basic;
                auth_detail = "HTTP basic".into();
                break;
            }
            other => {
                auth_ev = Evidence::Unsupported;
                auth_detail = format!("security scheme type `{other}` is not supported in v1 (use a pre-issued bearer token)");
            }
        }
    }
    if matches!(authentication, AuthSpec::None) && auth_ev == Evidence::Unsupported {
        notes.push(auth_detail.clone());
    }

    let paths = doc_json.get("paths").and_then(|p| p.as_object()).ok_or_else(|| MaError::validation("document has no paths"))?;
    let mut rate_limited_documented = false;
    let mut resources: Vec<ResourceSpec> = vec![];

    for (p, item) in paths {
        if p.contains('{') {
            continue; // item paths handled with their collection
        }
        let item = doc.resolve(item, 0);
        let Some(get) = item.get("get") else { continue };
        let resp = get.pointer("/responses/200").or_else(|| get.pointer("/responses/2XX")).map(|r| doc.resolve(r, 0));
        let Some(schema) = resp.and_then(|r| r.pointer("/content/application~1json/schema")) else {
            notes.push(format!("GET {p}: no JSON 200 response schema; skipped"));
            continue;
        };
        if get.pointer("/responses/429").is_some() {
            rate_limited_documented = true;
        }
        let schema = doc.resolve(schema, 0);
        // locate records array
        let (records_path, item_schema, page_props) = if schema.get("type").and_then(|t| t.as_str()) == Some("array") {
            (String::new(), schema.get("items").map(|i| doc.resolve(i, 0)), vec![])
        } else {
            let (props, _) = doc.props(schema);
            let arr = props.iter().find(|(_, s)| {
                s.get("type").and_then(|t| t.as_str()) == Some("array")
                    && s.get("items").map(|i| doc.resolve(i, 0)).is_some_and(|i| i.get("properties").is_some() || i.get("allOf").is_some())
            });
            match arr {
                Some((name, s)) => (name.clone(), s.get("items").map(|i| doc.resolve(i, 0)), props.clone()),
                None => {
                    notes.push(format!("GET {p}: response is not a collection; skipped"));
                    continue;
                }
            }
        };
        let Some(item_schema) = item_schema else { continue };
        let name = sanitize(p.rsplit('/').find(|s| !s.is_empty()).unwrap_or(p));
        let rel_path = p.clone();

        // fields
        let (props, required) = doc.props(item_schema);
        let mut fields = vec![];
        for (fname, fs) in &props {
            let (ft, nullable) = type_of(fs);
            let mut f = FieldDef::new(fname.clone(), ft);
            f.nullable = nullable || !required.contains(fname);
            f.required = required.contains(fname) && !nullable;
            f.read_only = fs.get("readOnly").and_then(|r| r.as_bool()).unwrap_or(false);
            f.custom = fname.starts_with("x_") || fname.starts_with("cf_") || fname.starts_with("custom_");
            f.description = fs.get("description").and_then(|d| d.as_str()).map(|d| {
                if is_suspicious(d) {
                    "[description withheld: it contained text addressed to AI agents or security settings]".to_string()
                } else {
                    d.chars().take(500).collect()
                }
            });
            f.enum_values = fs
                .get("enum")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            f.native_type = Some(format!(
                "{}{}",
                fs.get("type").map(|t| t.to_string().trim_matches('"').to_string()).unwrap_or_else(|| "object".into()),
                fs.get("format").and_then(|f| f.as_str()).map(|f| format!(":{f}")).unwrap_or_default()
            ));
            fields.push(f);
        }

        // query parameters
        let params: Vec<(String, &J)> = get
            .get("parameters")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| doc.resolve(x, 0))
                    .filter(|x| x.get("in").and_then(|i| i.as_str()) == Some("query"))
                    .filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(|n| (n.to_string(), x)))
                    .collect()
            })
            .unwrap_or_default();
        let find = |cands: &[&str]| params.iter().find(|(n, _)| cands.contains(&n.to_lowercase().as_str())).map(|(n, s)| (n.clone(), *s));
        let page_size_of = |s: Option<&J>| -> u32 {
            let sch = s.and_then(|s| s.get("schema"));
            let max = sch.and_then(|x| x.get("maximum")).and_then(|m| m.as_u64());
            let def = sch.and_then(|x| x.get("default")).and_then(|m| m.as_u64());
            max.map(|m| m.min(100)).or(def).unwrap_or(50).clamp(1, 1000) as u32
        };
        let limit = find(LIMIT_PARAMS);
        let next_field =
            page_props.iter().find(|(n, _)| NEXT_FIELDS.contains(&n.to_lowercase().as_str())).map(|(n, _)| n.clone()).or_else(|| {
                // one level nested: meta.next_cursor / pagination.next
                page_props.iter().find_map(|(n, s)| {
                    let (inner, _) = doc.props(s);
                    inner.iter().find(|(m, _)| NEXT_FIELDS.contains(&m.to_lowercase().as_str())).map(|(m, _)| format!("{n}.{m}"))
                })
            });
        let pagination = if let (Some((cp, _)), Some(nf)) = (find(CURSOR_PARAMS), next_field.clone()) {
            Pagination::Cursor {
                cursor_param: cp,
                next_cursor_path: nf,
                limit_param: limit.as_ref().map(|l| l.0.clone()),
                page_size: page_size_of(limit.map(|l| l.1)),
            }
        } else if let (Some((op, _)), Some((lp, ls))) = (find(OFFSET_PARAMS), limit.clone()) {
            Pagination::Offset { offset_param: op, limit_param: lp, page_size: page_size_of(Some(ls)) }
        } else if let Some((pp, _)) = find(PAGE_PARAMS) {
            Pagination::Page {
                page_param: pp,
                size_param: limit.as_ref().map(|l| l.0.clone()),
                page_size: page_size_of(limit.map(|l| l.1)),
                first_page: 1,
            }
        } else {
            notes.push(format!("GET {p}: no pagination parameters recognised; reading a single page"));
            Pagination::None
        };

        // item path and identity
        let item_path = paths.keys().find(|k| {
            k.starts_with(&format!("{p}/{{"))
                && k.ends_with('}')
                && k[p.len() + 1..].matches('{').count() == 1
                && !k[p.len() + 1..].contains('/')
        });
        let item_param = item_path.map(|k| k[p.len() + 2..k.len() - 1].to_string());
        let identity_field = item_param
            .clone()
            .filter(|ip| props.iter().any(|(n, _)| n == ip))
            .or_else(|| props.iter().find(|(n, _)| n == "id").map(|(n, _)| n.clone()))
            .or_else(|| {
                props
                    .iter()
                    .find(|(n, _)| {
                        required.contains(n)
                            && (n.ends_with("_id") || n.ends_with("_number") || n.ends_with("_key") || n.ends_with("_code"))
                    })
                    .map(|(n, _)| n.clone())
            })
            .unwrap_or_else(|| {
                notes.push(format!("{name}: no identity field found; using `id`"));
                "id".into()
            });

        let create = item.get("post").map(|post| {
            if post.pointer("/responses/429").is_some() {
                rate_limited_documented = true;
            }
            WriteOperation { method: HttpMethod::Post, path: rel_path.clone(), response_id_path: String::new(), idempotent: false }
        });
        let (get_op, update) =
            match item_path {
                Some(ip) => {
                    let it = doc.resolve(&paths[ip], 0);
                    let templ = format!("{p}/{{id}}");
                    (
                        it.get("get").map(|_| ItemOperation { path: templ.clone() }),
                        it.get("patch")
                            .map(|_| HttpMethod::Patch)
                            .or_else(|| it.get("put").map(|_| HttpMethod::Put))
                            .map(|m| WriteOperation { method: m, path: templ.clone(), response_id_path: String::new(), idempotent: true }),
                    )
                }
                None => (None, None),
            };
        let lookup = params.iter().find(|(n, _)| n == &identity_field).map(|(n, _)| LookupOperation {
            path: rel_path.clone(),
            query_param: n.clone(),
            key_field: identity_field.clone(),
            records_path: records_path.clone(),
        });

        resources.push(ResourceSpec {
            name,
            identity_field,
            list: Some(ListOperation { path: rel_path, records_path, pagination }),
            create,
            update,
            get: get_op,
            lookup,
            fields,
            relationships: vec![],
        });
    }

    // relationship inference across resources
    let idents: Vec<(String, String)> = resources.iter().map(|r| (r.name.clone(), r.identity_field.clone())).collect();
    for r in &mut resources {
        let mut rels = vec![];
        for f in &r.fields {
            if f.name == r.identity_field {
                continue;
            }
            for (other, ident) in &idents {
                if other == &r.name {
                    continue;
                }
                let singular = other.trim_end_matches('s');
                let desc = f.description.as_deref().unwrap_or("").to_lowercase();
                let by_name = f.name == *ident || f.name == format!("{singular}_id") || f.name == format!("{singular}_{ident}");
                let by_desc = ident != "id" && desc.contains(&ident.to_lowercase()) && !desc.is_empty();
                if by_name || by_desc {
                    rels.push(Relationship {
                        from_object: r.name.clone(),
                        from_field: f.name.clone(),
                        to_object: other.clone(),
                        to_field: ident.clone(),
                        evidence: Evidence::Inferred,
                    });
                    notes.push(format!(
                        "{}.{} inferred to reference {}.{} ({})",
                        r.name,
                        f.name,
                        other,
                        ident,
                        if by_name { "field name" } else { "field description" }
                    ));
                    break;
                }
            }
        }
        r.relationships = rels;
    }

    if resources.is_empty() {
        return Err(MaError::unsupported("no collection endpoints with JSON list responses were found"));
    }

    if !suspicious.is_empty() {
        notes.push(format!(
            "{} passage(s) in the document address AI agents or security settings; they were treated as inert data and had no effect",
            suspicious.len()
        ));
    }

    let spec = DeclarativeConnectorSpec {
        spec_version: CONNECTOR_SPEC_VERSION,
        connector_id: connector_id.to_string(),
        version: "1.0.0".into(),
        protocol: SpecProtocol::Rest,
        title: title.clone(),
        api_version: api_version.clone(),
        base_path,
        authentication,
        rate_limit: Some(RateLimitSpec { max_requests_per_second: 20, respect_retry_after: true }),
        resources,
        provenance: SpecProvenance {
            generated_by: concat!("connector-factory/openapi@", env!("CARGO_PKG_VERSION")).into(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            source_document_sha256: Some(source_sha256.to_string()),
            notes: notes.clone(),
        },
    };

    let mut manifest = CapabilityManifest::new(SystemIdentity {
        kind: "rest".into(),
        name: title,
        version: api_version,
        locator: format!("openapi sha256:{}", &source_sha256[..16.min(source_sha256.len())]),
    });
    let any = |f: &dyn Fn(&ResourceSpec) -> bool| spec.resources.iter().any(f);
    let doc_or = |b: bool| if b { Evidence::Documented } else { Evidence::Unsupported };
    manifest
        .set("schema_discovery", Evidence::Documented, Some("OpenAPI component schemas".into()))
        .set("read", doc_or(any(&|r| r.list.is_some())), None)
        .set("write", doc_or(any(&|r| r.create.is_some())), None)
        .set("update", doc_or(any(&|r| r.update.is_some())), None)
        .set("lookup_by_key", doc_or(any(&|r| r.lookup.is_some())), None)
        .set("delete", Evidence::Unsupported, Some("never generated".into()))
        .set(
            "pagination",
            if any(&|r| !matches!(r.list.as_ref().map(|l| &l.pagination), Some(Pagination::None) | None)) {
                Evidence::Documented
            } else {
                Evidence::Unknown
            },
            None,
        )
        .set("authentication", auth_ev, Some(auth_detail))
        .set(
            "rate_limits",
            if rate_limited_documented { Evidence::Documented } else { Evidence::Unknown },
            Some("HTTP 429 with Retry-After".into()),
        )
        .set("transactions", Evidence::Unsupported, None)
        .set("attachments", Evidence::Unknown, None)
        .set("history", Evidence::Unknown, None);
    for r in &spec.resources {
        manifest.set_object(&r.name, "read", doc_or(r.list.is_some()), None);
        manifest.set_object(&r.name, "create", doc_or(r.create.is_some()), None);
        manifest.set_object(&r.name, "lookup", doc_or(r.lookup.is_some()), None);
        if let Some(l) = &r.list {
            manifest.set_object(&r.name, "pagination", Evidence::Documented, Some(format!("{:?}", l.pagination)));
        }
    }
    manifest.limitations = notes.clone();
    let validation_errors = crate::validate::validate_spec(&spec);
    Ok(GenerationResult { spec, manifest, notes, suspicious_text: suspicious, validation_errors })
}
