//! Generic REST runtime. It interprets a validated [`DeclarativeConnectorSpec`]; the spec can only
//! select behaviours implemented here. Base URL, credentials and network policy come from the
//! operator-controlled connection, never from the spec.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use ma_connector_sdk::{Connector, ConnectorHandle, RecordLookup, RecordSink, RecordSource, SinkObjectSpec, WriteRequest, WriteResult};
use ma_security::network::NetworkPolicy;
use ma_types::*;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;
use url::Url;

const MAX_ATTEMPTS: u32 = 6;
const MAX_BODY: usize = 32 * 1024 * 1024;

pub struct RestConnector {
    spec: DeclarativeConnectorSpec,
    base: String,
    base_url: Url,
    client: reqwest::Client,
    policy: NetworkPolicy,
    credential: Option<String>,
    next_slot: tokio::sync::Mutex<Instant>,
    sinks: Mutex<HashMap<String, SinkObjectSpec>>,
    stats: Mutex<BTreeMap<String, u64>>,
}

#[derive(serde::Deserialize)]
struct RestSettings {
    base_url: String,
    #[serde(default)]
    allow_private_network: bool,
}

impl RestConnector {
    /// `settings`: `{"base_url": "https://api.example.com", "allow_private_network": false}`.
    pub async fn connect(
        spec: DeclarativeConnectorSpec,
        settings: &serde_json::Value,
        allowed_hosts: &[String],
        credential: Option<String>,
    ) -> MaResult<Self> {
        let s: RestSettings =
            serde_json::from_value(settings.clone()).map_err(|e| MaError::config(format!("invalid REST settings: {e}")))?;
        let policy = NetworkPolicy::new(allowed_hosts.to_vec(), s.allow_private_network);
        let base_url = Url::parse(&s.base_url).map_err(|e| MaError::config(format!("invalid base_url: {e}")))?;
        let client = policy.client_for(&base_url).await?;
        if !matches!(spec.authentication, AuthSpec::None) && credential.is_none() {
            return Err(MaError::config("this connector requires a credential (set credential_ref on the connection)"));
        }
        let base = format!("{}{}", s.base_url.trim_end_matches('/'), spec.base_path.trim_end_matches('/'));
        Ok(RestConnector {
            spec,
            base,
            base_url,
            client,
            policy,
            credential,
            next_slot: tokio::sync::Mutex::new(Instant::now()),
            sinks: Mutex::new(HashMap::new()),
            stats: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn into_handle(self) -> ConnectorHandle {
        let a = Arc::new(self);
        ConnectorHandle { connector: a.clone(), source: Some(a.clone()), sink: Some(a.clone()), lookup: Some(a) }
    }

    pub fn spec(&self) -> &DeclarativeConnectorSpec {
        &self.spec
    }

    /// Request statistics (`requests`, `rate_limited`, `retries`).
    pub fn stats(&self) -> BTreeMap<String, u64> {
        self.stats.lock().expect("lock").clone()
    }

    fn bump(&self, k: &str) {
        *self.stats.lock().expect("lock").entry(k.to_string()).or_default() += 1;
    }

    fn resource(&self, object: &str) -> MaResult<&ResourceSpec> {
        self.spec.resource(object).ok_or_else(|| MaError::not_found(format!("connector has no resource `{object}`")))
    }

    fn url(&self, path: &str, id: Option<&str>, query: &[(String, String)]) -> MaResult<Url> {
        let mut p = path.to_string();
        if let Some(id) = id {
            p = p.replace("{id}", &utf8_percent_encode(id, NON_ALPHANUMERIC).to_string());
        }
        if !p.starts_with('/') || p.contains("://") || p.contains("..") || p.contains('{') {
            return Err(MaError::policy(format!("refusing unsafe request path `{p}`")));
        }
        let mut u = Url::parse(&format!("{}{}", self.base, p)).map_err(|e| MaError::config(format!("bad URL: {e}")))?;
        if u.host_str() != self.base_url.host_str() || u.port_or_known_default() != self.base_url.port_or_known_default() {
            return Err(MaError::policy("request would leave the configured host"));
        }
        self.policy.check_url_static(&u)?;
        if !query.is_empty() {
            let mut qp = u.query_pairs_mut();
            for (k, v) in query {
                qp.append_pair(k, v);
            }
        }
        Ok(u)
    }

    async fn throttle(&self) {
        let Some(rl) = &self.spec.rate_limit else { return };
        if rl.max_requests_per_second == 0 {
            return;
        }
        let interval = Duration::from_secs_f64(1.0 / rl.max_requests_per_second as f64);
        let mut slot = self.next_slot.lock().await;
        let now = Instant::now();
        if *slot > now {
            tokio::time::sleep(*slot - now).await;
        }
        *slot = Instant::now().max(*slot) + interval;
    }

    /// Execute one request with retries for 429/5xx/transport errors. Returns (status, body).
    ///
    /// `replayable` says whether the request may be sent again when its outcome is unknown
    /// (a 5xx answer, or a connection that failed after the request left). Reads and operations
    /// documented as idempotent are; a plain create is not — re-sending it could create a second
    /// record, so the error is returned instead. Rate-limit answers (429/503) and connection
    /// failures before anything was sent are always retried: the server did not act on them.
    async fn send(&self, method: HttpMethod, url: Url, body: Option<Vec<u8>>, replayable: bool) -> MaResult<(u16, Value)> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.throttle().await;
            let m = match method {
                HttpMethod::Get => reqwest::Method::GET,
                HttpMethod::Post => reqwest::Method::POST,
                HttpMethod::Put => reqwest::Method::PUT,
                HttpMethod::Patch => reqwest::Method::PATCH,
                HttpMethod::Delete => return Err(MaError::policy("DELETE is not permitted by the REST runtime")),
            };
            let mut rb = self.client.request(m, url.clone()).header("Accept", "application/json");
            if let Some(b) = &body {
                rb = rb.header("Content-Type", "application/json").body(b.clone());
            }
            rb = match (&self.spec.authentication, &self.credential) {
                (AuthSpec::None, _) => rb,
                (AuthSpec::Bearer, Some(c)) => rb.header("Authorization", format!("Bearer {c}")),
                (AuthSpec::ApiKeyHeader { header }, Some(c)) => rb.header(header.as_str(), c.as_str()),
                (AuthSpec::Basic, Some(c)) => {
                    rb.header("Authorization", format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(c)))
                }
                (_, None) => return Err(MaError::config("credential missing")),
            };
            self.bump("requests");
            let resp = match rb.send().await {
                Ok(r) => r,
                Err(e) => {
                    if (replayable || e.is_connect()) && attempt < MAX_ATTEMPTS {
                        self.bump("retries");
                        tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
                        continue;
                    }
                    let unknown =
                        if replayable || e.is_connect() { "" } else { " (not re-sent: the write may or may not have been applied)" };
                    return Err(MaError::transient(format!("request failed: {}{unknown}", without_url(&e))));
                }
            };
            let status = resp.status().as_u16();
            if status == 429 || status == 503 {
                self.bump("rate_limited");
                let ra = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs);
                let respect = self.spec.rate_limit.as_ref().map(|r| r.respect_retry_after).unwrap_or(true);
                if attempt < MAX_ATTEMPTS {
                    self.bump("retries");
                    let wait =
                        if respect { ra.unwrap_or(Duration::from_millis(250 << attempt)) } else { Duration::from_millis(250 << attempt) };
                    tokio::time::sleep(wait.min(Duration::from_secs(30))).await;
                    continue;
                }
                return Err(MaError::rate_limited(format!("rate limited (HTTP {status})"), ra));
            }
            if status >= 500 && replayable && attempt < MAX_ATTEMPTS {
                self.bump("retries");
                tokio::time::sleep(Duration::from_millis(200 << attempt)).await;
                continue;
            }
            let bytes = resp.bytes().await.map_err(|e| MaError::transient(format!("reading response: {}", without_url(&e))))?;
            if bytes.len() > MAX_BODY {
                return Err(MaError::validation("response body too large"));
            }
            let parsed = if bytes.is_empty() {
                Value::Null
            } else {
                json_text::parse(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).into_owned()))
            };
            return match status {
                200..=299 => Ok((status, parsed)),
                401 | 403 => Err(MaError::auth(format!("HTTP {status}: credentials rejected"))),
                404 => Err(MaError::not_found(format!("HTTP 404 for {}", url.path()))),
                409 => Err(MaError::new(ErrorClass::Validation, format!("HTTP 409 conflict: {}", excerpt(&parsed)))),
                400..=499 => Err(MaError::validation(format!("HTTP {status}: {}", excerpt(&parsed)))),
                _ if replayable => Err(MaError::transient(format!("HTTP {status}: {}", excerpt(&parsed)))),
                _ => Err(MaError::transient(format!(
                    "HTTP {status}: {} (not re-sent: the write may or may not have been applied)",
                    excerpt(&parsed)
                ))),
            };
        }
    }

    fn records_at<'v>(body: &'v Value, path: &str) -> MaResult<&'v Vec<Value>> {
        let node = if path.is_empty() { Some(body) } else { body.get_path(&path.split('.').collect::<Vec<_>>()) };
        match node {
            Some(Value::Array(a)) => Ok(a),
            _ => Err(MaError::validation(format!("response does not contain an array at `{path}`"))),
        }
    }

    fn to_record(&self, res: &ResourceSpec, item: &Value) -> MaResult<Record> {
        let Value::Object(map) = item else { return Err(MaError::validation("list item is not an object")) };
        let id = map
            .get(&res.identity_field)
            .and_then(|v| v.to_text())
            .ok_or_else(|| MaError::validation(format!("record lacks identity field `{}`", res.identity_field)))?;
        let mut r = Record::new(&res.name, id);
        r.fields = map.clone();
        r.provenance.system = self.spec.title.clone();
        r.provenance.extracted_at = Some(chrono::Utc::now());
        Ok(r)
    }

    async fn find_by_key(&self, res: &ResourceSpec, key_field: &str, key: &str) -> MaResult<Option<Record>> {
        let lk = res.lookup.as_ref().ok_or_else(|| MaError::unsupported(format!("`{}` has no lookup operation", res.name)))?;
        if lk.key_field != key_field {
            return Err(MaError::unsupported(format!("lookup supports `{}`, not `{key_field}`", lk.key_field)));
        }
        let url = self.url(&lk.path, None, &[(lk.query_param.clone(), key.to_string())])?;
        let (_, body) = self.send(HttpMethod::Get, url, None, true).await?;
        for item in Self::records_at(&body, &lk.records_path)? {
            if item.get_path(&[key_field]).and_then(|v| v.to_text()).as_deref() == Some(key) {
                return Ok(Some(self.to_record(res, item)?));
            }
        }
        Ok(None)
    }

    fn body_for(fields: &BTreeMap<String, Value>) -> Vec<u8> {
        let mut out = vec![];
        json_text::write_object(fields.iter(), &mut out);
        out
    }

    fn response_id(res: &ResourceSpec, op: &WriteOperation, body: &Value, fallback: &str) -> String {
        let path = if op.response_id_path.is_empty() { res.identity_field.clone() } else { op.response_id_path.clone() };
        body.get_path(&path.split('.').collect::<Vec<_>>()).and_then(|v| v.to_text()).unwrap_or_else(|| fallback.to_string())
    }

    async fn write_one(&self, res: &ResourceSpec, spec: &SinkObjectSpec, r: &WriteRequest) -> MaResult<String> {
        let key = spec.key_field.as_ref().and_then(|k| r.fields.get(k).and_then(|v| v.to_text()).map(|v| (k.clone(), v)));
        if spec.mode == WriteMode::Upsert
            && res.lookup.is_some()
            && let Some((kf, kv)) = &key
            && let Some(existing) = self.find_by_key(res, kf, kv).await?
        {
            if let Some(up) = &res.update {
                let url = self.url(&up.path, Some(&existing.id), &[])?;
                self.send(up.method, url, Some(Self::body_for(&r.fields)), up.idempotent || up.method == HttpMethod::Put).await?;
            }
            return Ok(existing.id);
        }
        let create = res.create.as_ref().ok_or_else(|| MaError::unsupported(format!("`{}` cannot be created", res.name)))?;
        let url = self.url(&create.path, None, &[])?;
        match self.send(create.method, url, Some(Self::body_for(&r.fields)), create.idempotent).await {
            Ok((_, body)) => Ok(Self::response_id(res, create, &body, &key.map(|k| k.1).unwrap_or_default())),
            Err(e) if e.message.starts_with("HTTP 409") && res.lookup.is_some() => match &key {
                Some((kf, kv)) => match self.find_by_key(res, kf, kv).await? {
                    Some(found) => Ok(found.id),
                    None => Err(e),
                },
                None => Err(e),
            },
            Err(e) => Err(e),
        }
    }
}

fn without_url(e: &reqwest::Error) -> String {
    // reqwest errors include the URL; query strings could carry identifiers, keep it short.
    let mut s = e.to_string();
    if let Some(u) = e.url() {
        s = s.replace(u.as_str(), u.path());
    }
    s
}

fn excerpt(v: &Value) -> String {
    let s = json_text::to_plain_string(v);
    if s.len() > 300 {
        format!("{}…", &s[..s.char_indices().take_while(|(i, _)| *i < 300).last().map(|(i, _)| i).unwrap_or(0)])
    } else {
        s
    }
}

#[async_trait]
impl Connector for RestConnector {
    fn identify(&self) -> SystemIdentity {
        SystemIdentity {
            kind: "rest".into(),
            name: self.spec.title.clone(),
            version: self.spec.api_version.clone(),
            locator: self.base.clone(),
        }
    }

    fn capabilities(&self) -> CapabilityManifest {
        let mut m = CapabilityManifest::new(self.identify());
        let any = |f: &dyn Fn(&ResourceSpec) -> bool| self.spec.resources.iter().any(f);
        let ev = |b: bool| if b { Evidence::Documented } else { Evidence::Unsupported };
        m.set("read", ev(any(&|r| r.list.is_some())), None)
            .set("write", ev(any(&|r| r.create.is_some())), None)
            .set("update", ev(any(&|r| r.update.is_some())), None)
            .set("lookup_by_key", ev(any(&|r| r.lookup.is_some())), None)
            .set("delete", Evidence::Unsupported, Some("the REST runtime never issues DELETE".into()))
            .set("transactions", Evidence::Unsupported, None)
            .set("schema_discovery", Evidence::Documented, Some("from the connector specification".into()))
            .set("rate_limits", if self.spec.rate_limit.is_some() { Evidence::Documented } else { Evidence::Unknown }, None);
        for r in &self.spec.resources {
            m.set_object(&r.name, "read", ev(r.list.is_some()), None);
            m.set_object(&r.name, "create", ev(r.create.is_some()), None);
        }
        m
    }

    async fn check_connection(&self) -> MaResult<()> {
        let res = self.spec.resources.iter().find(|r| r.list.is_some()).ok_or_else(|| MaError::unsupported("no readable resource"))?;
        self.read_batch(&res.name, None, 1).await.map(|_| ())
    }

    async fn discover_schema(&self) -> MaResult<SystemSchema> {
        Ok(SystemSchema {
            objects: self
                .spec
                .resources
                .iter()
                .map(|r| ObjectDef {
                    name: r.name.clone(),
                    fields: r.fields.clone(),
                    primary_key: vec![r.identity_field.clone()],
                    unique_keys: vec![],
                    estimated_count: None,
                    readable: r.list.is_some(),
                    writable: r.create.is_some(),
                })
                .collect(),
            relationships: self.spec.resources.iter().flat_map(|r| r.relationships.clone()).collect(),
        })
    }

    fn connector_version(&self) -> String {
        self.spec.version.clone()
    }
}

#[async_trait]
impl RecordSource for RestConnector {
    async fn read_batch(&self, object: &str, cursor: Option<&Cursor>, limit: usize) -> MaResult<Batch> {
        let res = self.resource(object)?;
        let list = res.list.as_ref().ok_or_else(|| MaError::unsupported(format!("`{object}` cannot be listed")))?;
        let mut q: Vec<(String, String)> = vec![];
        let (page_size, next): (usize, Box<dyn Fn(&Value, usize) -> Option<Cursor> + Send + Sync>) = match &list.pagination {
            Pagination::None => (usize::MAX, Box::new(|_, _| None)),
            Pagination::Cursor { cursor_param, next_cursor_path, limit_param, page_size } => {
                let n = (*page_size as usize).min(limit).max(1);
                if let Some(lp) = limit_param {
                    q.push((lp.clone(), n.to_string()));
                }
                if let Some(c) = cursor.and_then(|c| c.get("cursor")).and_then(|v| v.as_str()) {
                    q.push((cursor_param.clone(), c.to_string()));
                }
                let path: Vec<String> = next_cursor_path.split('.').map(String::from).collect();
                (
                    n,
                    Box::new(move |body: &Value, count: usize| {
                        if count == 0 {
                            return None;
                        }
                        let p: Vec<&str> = path.iter().map(String::as_str).collect();
                        match body.get_path(&p) {
                            Some(Value::Null) | None => None,
                            Some(v) => v.to_text().filter(|s| !s.is_empty()).map(|s| json!({ "cursor": s })),
                        }
                    }),
                )
            }
            Pagination::Offset { offset_param, limit_param, page_size } => {
                let n = (*page_size as usize).min(limit).max(1);
                let off = cursor.and_then(|c| c.get("offset")).and_then(|v| v.as_u64()).unwrap_or(0);
                q.push((offset_param.clone(), off.to_string()));
                q.push((limit_param.clone(), n.to_string()));
                (n, Box::new(move |_, count| (count >= n).then(|| json!({ "offset": off + count as u64 }))))
            }
            Pagination::Page { page_param, size_param, page_size, first_page } => {
                let n = (*page_size as usize).max(1);
                let page = cursor.and_then(|c| c.get("page")).and_then(|v| v.as_u64()).unwrap_or(*first_page as u64);
                q.push((page_param.clone(), page.to_string()));
                if let Some(sp) = size_param {
                    q.push((sp.clone(), n.to_string()));
                }
                (n, Box::new(move |_, count| (count >= n).then(|| json!({ "page": page + 1 }))))
            }
        };
        let _ = page_size;
        let url = self.url(&list.path, None, &q)?;
        let (_, body) = self.send(HttpMethod::Get, url, None, true).await?;
        let items = Self::records_at(&body, &list.records_path)?;
        let records = items.iter().map(|i| self.to_record(res, i)).collect::<MaResult<Vec<_>>>()?;
        let next_cursor = next(&body, records.len());
        Ok(Batch { records, next_cursor })
    }

    async fn count(&self, _object: &str) -> MaResult<Option<u64>> {
        Ok(None)
    }
}

#[async_trait]
impl RecordSink for RestConnector {
    async fn prepare(&self, spec: &SinkObjectSpec, _resume: Option<&Cursor>) -> MaResult<()> {
        let res = self.resource(&spec.object)?;
        if res.create.is_none() {
            return Err(MaError::unsupported(format!("`{}` has no create operation", spec.object)));
        }
        if spec.mode == WriteMode::Upsert && (spec.key_field.is_none() || res.lookup.is_none()) {
            return Err(MaError::config(format!(
                "upsert into `{}` needs a key field and a lookup operation; use insert mode",
                spec.object
            )));
        }
        self.sinks.lock().expect("lock").insert(spec.object.clone(), spec.clone());
        Ok(())
    }

    async fn write_batch(&self, object: &str, records: Vec<WriteRequest>) -> MaResult<Vec<WriteResult>> {
        let res = self.resource(object)?;
        let spec = self.sinks.lock().expect("lock").get(object).cloned().ok_or_else(|| MaError::internal("sink not prepared"))?;
        let mut out = Vec::with_capacity(records.len());
        for (i, r) in records.iter().enumerate() {
            match self.write_one(res, &spec, r).await {
                Err(e) if i == 0 && matches!(e.class, ErrorClass::Auth | ErrorClass::Policy | ErrorClass::Config) => return Err(e),
                other => out.push(other),
            }
        }
        Ok(out)
    }

    async fn durable_token(&self, _object: &str) -> MaResult<Option<Cursor>> {
        Ok(None)
    }

    async fn update_fields(&self, object: &str, dest_id: &str, fields: BTreeMap<String, Value>) -> MaResult<()> {
        let res = self.resource(object)?;
        let up = res.update.as_ref().ok_or_else(|| MaError::unsupported("no update operation"))?;
        let url = self.url(&up.path, Some(dest_id), &[])?;
        self.send(up.method, url, Some(Self::body_for(&fields)), up.idempotent || up.method == HttpMethod::Put).await.map(|_| ())
    }

    async fn finish(&self, _object: &str) -> MaResult<()> {
        Ok(())
    }

    fn idempotent_rewrites(&self, mode: WriteMode, has_key: bool) -> bool {
        // Only when every upsert first looks the key up; plain POSTs are not idempotent.
        mode == WriteMode::Upsert && has_key && self.spec.resources.iter().all(|r| r.create.is_none() || r.lookup.is_some())
    }

    fn supports_updates(&self) -> bool {
        self.spec.resources.iter().any(|r| r.update.is_some())
    }
}

#[async_trait]
impl RecordLookup for RestConnector {
    async fn lookup_by_key(&self, object: &str, key_field: &str, key: &str) -> MaResult<Option<Record>> {
        let res = self.resource(object)?;
        self.find_by_key(res, key_field, key).await
    }
}
