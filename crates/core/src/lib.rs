//! Universal application core.
//!
//! Every user-facing operation is a method on [`App`]. The CLI and the MCP server are thin
//! protocol adapters over these methods; neither contains migration logic.

pub mod consent;
pub mod requests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ma_connector_factory as factory;
use ma_connector_sdk::ConnectorHandle;
use ma_execution::{JobRunner, RunEnd};
use ma_mapping::{ExtensionRunner, SuggestOptions, SuggestionSet};
use ma_persistence::Store;
use ma_security::keys::{self, Key};
use ma_types::*;
use serde_json::json;

pub use consent::{Challenge, InteractiveSession, OperatorConsent};
pub use requests::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILTIN: &str = "builtin";

pub struct App {
    pub store: Store,
    home: PathBuf,
    master_key: Key,
    approval_key: Key,
}

/// Who is calling: used for audit events. Agents are always recorded as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Cli,
    /// The operator working through the local web UI.
    Ui,
    Mcp,
    Worker,
}

impl Actor {
    pub fn label(self) -> String {
        match self {
            Actor::Cli => format!("cli:{}", ma_security::os_operator()),
            Actor::Ui => format!("ui:{}", ma_security::os_operator()),
            Actor::Mcp => "mcp-agent".into(),
            Actor::Worker => format!("worker:{}", std::process::id()),
        }
    }
}

impl App {
    /// `MA_HOME`, else `~/.migrate-anything`.
    pub fn default_home() -> PathBuf {
        if let Ok(h) = std::env::var("MA_HOME") {
            return PathBuf::from(h);
        }
        let base = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
        Path::new(&base).join(".migrate-anything")
    }

    pub async fn open(home: &Path) -> MaResult<App> {
        std::fs::create_dir_all(home.join("keys"))?;
        std::fs::create_dir_all(home.join("logs"))?;
        let store = Store::open(&home.join("state.db"), "default").await?;
        let master_key = keys::load_or_create(&home.join("keys").join("secrets.key"))?;
        let approval_key = keys::load_or_create(&home.join("keys").join("approvals.key"))?;
        Ok(App { store, home: home.to_path_buf(), master_key, approval_key })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    // ================================================================= doctor
    pub async fn doctor(&self) -> MaResult<DoctorReport> {
        let mut checks = vec![];
        let mut add = |name: &str, ok: bool, detail: String| checks.push(DoctorCheck { name: name.into(), ok, detail });
        add("version", true, VERSION.into());
        add("home", true, self.home.display().to_string());
        match self.store.schema_version().await {
            Ok(v) => add("state database", true, format!("SQLite, schema version {v}")),
            Err(e) => add("state database", false, e.message),
        }
        add("secret key", self.home.join("keys/secrets.key").exists(), "ChaCha20-Poly1305 key present".into());
        add("approval key", self.home.join("keys/approvals.key").exists(), "HMAC-SHA256 key present".into());
        match ma_sandbox::Sandbox::new(Default::default()).and_then(|sb| {
            sb.load("doctor", ma_sandbox::UPPERCASE_WAT.as_bytes())?;
            sb.run("doctor", &json!("ok"))
        }) {
            Ok(v) if v == json!("OK") => add("wasm sandbox", true, "fuel, memory and deadline limits active; zero host imports".into()),
            Ok(v) => add("wasm sandbox", false, format!("unexpected self-test output {v}")),
            Err(e) => add("wasm sandbox", false, e.message),
        }
        let conns = self.store.list_connections().await.map(|c| c.len()).unwrap_or(0);
        add("connections", true, format!("{conns} configured"));
        let ok = checks.iter().all(|c| c.ok);
        Ok(DoctorReport { ok, checks })
    }

    // ================================================================= connections & secrets
    pub async fn add_connection(&self, c: ConnectionConfig, actor: Actor) -> MaResult<()> {
        if c.name.is_empty() || c.name.len() > 64 || !c.name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            return Err(MaError::validation("connection name must be 1-64 chars of [A-Za-z0-9_-]"));
        }
        if actor == Actor::Mcp {
            // Agents may register new, credential-less connections only. Attaching a stored
            // credential or changing where an existing connection points is an operator action:
            // otherwise an agent could route a secret to a host of its choosing, or retarget an
            // approved plan.
            if c.credential_ref.is_some() {
                return Err(MaError::policy(format!(
                    "credentials can only be attached by the operator: migrate-anything connections add {} ... --credential-ref <name>",
                    c.name
                )));
            }
            if self.store.get_connection(&c.name).await.is_ok() {
                return Err(MaError::policy(format!(
                    "connection `{}` already exists; only the operator can change it (migrate-anything connections add)",
                    c.name
                )));
            }
        }
        let mut c = c;
        match c.kind.as_str() {
            "files" => {
                // Store an absolute path: a relative one would silently point somewhere else
                // when a worker or the MCP server runs from a different directory.
                let p = c
                    .settings
                    .get("path")
                    .and_then(|p| p.as_str())
                    .ok_or_else(|| MaError::validation("file connections need settings.path (a folder)"))?;
                let abs = std::path::absolute(p).map_err(|e| MaError::validation(format!("invalid path `{p}`: {e}")))?;
                c.settings["path"] = json!(abs.to_string_lossy());
            }
            "postgres" => {}
            "rest" => {
                let base = c
                    .settings
                    .get("base_url")
                    .and_then(|b| b.as_str())
                    .ok_or_else(|| MaError::validation("rest connections need settings.base_url"))?;
                let u = url::Url::parse(base).map_err(|e| MaError::validation(format!("invalid base_url: {e}")))?;
                let policy = ma_security::network::NetworkPolicy::new(c.allowed_hosts.clone(), false);
                policy.check_url_static(&u).map_err(|e| MaError::validation(format!("{}; add the host to allowed_hosts", e.message)))?;
            }
            other => return Err(MaError::validation(format!("unknown connector kind `{other}` (files|postgres|rest)"))),
        }
        if let Some(u) = c.settings.get("url").and_then(|u| u.as_str())
            && url::Url::parse(u).ok().and_then(|u| u.password().map(|_| ())).is_some()
        {
            return Err(MaError::validation(
                "do not put passwords in connection URLs; store them with `credentials set` and use credential_ref",
            ));
        }
        self.store.put_connection(&c).await?;
        self.store.audit(&actor.label(), "connection.saved", Some(&c.name), json!({"kind": c.kind, "environment": c.environment})).await
    }

    /// Store a secret encrypted at rest. Not exposed over MCP: agents never handle secrets.
    pub async fn set_credential(&self, name: &str, secret: &str) -> MaResult<()> {
        let ct = ma_security::secrets::encrypt(&self.master_key, name, secret)?;
        self.store.put_credential(name, &ct).await?;
        self.store.audit(&Actor::Cli.label(), "credential.saved", Some(name), json!({})).await
    }

    async fn credential(&self, name: Option<&str>) -> MaResult<Option<String>> {
        match name {
            None => Ok(None),
            Some(n) => {
                let ct = self.store.get_credential(n).await?;
                Ok(Some(ma_security::secrets::decrypt(&self.master_key, n, &ct)?))
            }
        }
    }

    /// Construct a connector for a connection. Generated connectors are re-validated on load.
    pub async fn connect(&self, name: &str) -> MaResult<(ConnectionConfig, ConnectorHandle, ConnectorRef)> {
        let c = self.store.get_connection(name).await?;
        let builtin =
            |kind: &str| ConnectorRef { kind: kind.into(), id: format!("{BUILTIN}-{kind}"), version: VERSION.into(), generated: false };
        match c.kind.as_str() {
            "files" => {
                let h = ma_connector_files::FileConnector::new(&c.settings)?.into_handle();
                Ok((c, h, builtin("files")))
            }
            "postgres" => {
                let pw = self.credential(c.credential_ref.as_deref()).await?;
                let pg = ma_connector_postgres::PostgresConnector::connect(&c.settings, pw).await?;
                Ok((c, ma_connector_postgres::handle(pg), builtin("postgres")))
            }
            "rest" => {
                let r = c.connector_ref.clone().ok_or_else(|| {
                    MaError::config(format!("connection `{name}` has no connector yet; generate one with `connectors generate`"))
                })?;
                let (id, ver) = r.split_once('@').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((r.clone(), None));
                let row = self.store.get_connector_version(&id, ver.as_deref()).await?;
                if row.status == ConnectorStatus::Disabled {
                    return Err(MaError::policy(format!("connector {r} is disabled")));
                }
                let errs = factory::validate_spec(&row.spec);
                if !errs.is_empty() {
                    return Err(MaError::policy(format!("connector {r} failed validation: {}", errs.join("; "))));
                }
                if ma_types::hash::canonical_sha256(&row.spec) != row.spec_sha256 {
                    return Err(MaError::policy(format!("connector {r} was modified after registration")));
                }
                let cred = self.credential(c.credential_ref.as_deref()).await?;
                let version = row.spec.version.clone();
                let rest = ma_connector_rest::RestConnector::connect(row.spec, &c.settings, &c.allowed_hosts, cred).await?;
                Ok((c, rest.into_handle(), ConnectorRef { kind: "rest".into(), id, version, generated: true }))
            }
            other => Err(MaError::config(format!("unknown connector kind {other}"))),
        }
    }

    // ================================================================= discovery
    pub async fn discover(&self, connection: &str, actor: Actor) -> MaResult<DiscoveryResult> {
        let (_, h, cref) = self.connect(connection).await?;
        h.connector.check_connection().await?;
        let schema = h.connector.discover_schema().await?;
        let mut manifest = h.connector.capabilities();
        if cref.generated
            && let Ok(row) = self.store.get_connector_version(&cref.id, Some(&cref.version)).await
        {
            // registry manifest carries contract-test evidence
            manifest = row.manifest;
            manifest.identity = h.connector.identify();
        }
        let identity = h.connector.identify();
        self.store.put_system(connection, &identity, &manifest).await?;
        self.store.put_schema(connection, &schema).await?;
        self.store.audit(&actor.label(), "system.discovered", Some(connection), json!({"objects": schema.objects.len()})).await?;
        Ok(DiscoveryResult { connection: connection.into(), identity, manifest, schema, connector: cref })
    }

    /// Statistics-only profiling. Raw values are not returned unless `include_examples` is set,
    /// so record data does not flow to an AI client by default.
    pub async fn analyze_schema(
        &self,
        connection: &str,
        objects: &[String],
        sample: usize,
        include_examples: bool,
    ) -> MaResult<SchemaAnalysis> {
        let d = self.discover(connection, Actor::Cli).await?;
        let (_, h, _) = self.connect(connection).await?;
        let src = h.source()?;
        let mut profiles = vec![];
        for o in d.schema.objects.iter().filter(|o| objects.is_empty() || objects.contains(&o.name)) {
            if !o.readable {
                continue;
            }
            let b = src.read_batch(&o.name, None, sample.clamp(1, 10_000)).await?;
            let mut fields = vec![];
            for f in &o.fields {
                let mut p = FieldProfile {
                    field: f.name.clone(),
                    declared_type: f.field_type,
                    nulls: 0,
                    missing: 0,
                    invalid: 0,
                    distinct_in_sample: 0,
                    max_length: 0,
                    examples: vec![],
                };
                let mut distinct = std::collections::BTreeSet::new();
                for r in &b.records {
                    match r.fields.get(&f.name) {
                        None => p.missing += 1,
                        Some(Value::Null) => p.nulls += 1,
                        Some(v) => {
                            if v.is_problem() {
                                p.invalid += 1;
                            }
                            let t = v.to_text().unwrap_or_default();
                            p.max_length = p.max_length.max(t.chars().count());
                            if distinct.len() < 1000 {
                                distinct.insert(t.clone());
                            }
                            if include_examples && p.examples.len() < 3 && !p.examples.contains(&t) {
                                p.examples.push(t.chars().take(80).collect());
                            }
                        }
                    }
                }
                p.distinct_in_sample = distinct.len();
                fields.push(p);
            }
            profiles.push(ObjectProfile { object: o.name.clone(), sampled: b.records.len(), fields });
        }
        Ok(SchemaAnalysis { discovery: d, profiles, examples_included: include_examples })
    }

    // ================================================================= connector factory
    async fn fetch_document(&self, source: &DocumentSource) -> MaResult<Vec<u8>> {
        match source {
            DocumentSource::File { path } => {
                let meta = std::fs::metadata(path).map_err(|e| MaError::not_found(format!("{path}: {e}")))?;
                if meta.len() > 20 * 1024 * 1024 {
                    return Err(MaError::validation("document too large"));
                }
                Ok(std::fs::read(path)?)
            }
            DocumentSource::Url { url, connection } => {
                let c = self.store.get_connection(connection).await?;
                let allow_private = c.settings.get("allow_private_network").and_then(|v| v.as_bool()).unwrap_or(false);
                let policy = ma_security::network::NetworkPolicy::new(c.allowed_hosts.clone(), allow_private);
                let u = url::Url::parse(url).map_err(|e| MaError::validation(format!("invalid URL: {e}")))?;
                let client = policy.client_for(&u).await?;
                let resp = client.get(u).send().await.map_err(|e| MaError::transient(format!("fetching document: {e}")))?;
                if !resp.status().is_success() {
                    return Err(MaError::validation(format!("document request returned HTTP {}", resp.status())));
                }
                const MAX_DOC: usize = 20 * 1024 * 1024;
                if resp.content_length().is_some_and(|n| n > MAX_DOC as u64) {
                    return Err(MaError::validation("API document larger than 20 MiB"));
                }
                let mut resp = resp;
                let mut body = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(|e| MaError::transient(format!("reading document: {e}")))? {
                    if body.len() + chunk.len() > MAX_DOC {
                        return Err(MaError::validation("API document larger than 20 MiB; download aborted"));
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(body)
            }
        }
    }

    pub async fn generate_connector(&self, req: &GenerateConnectorRequest, actor: Actor) -> MaResult<GenerateConnectorResponse> {
        // 1. registry first: reuse an existing connector generated from the identical document
        let bytes = self.fetch_document(&req.source).await?;
        let sha = ma_types::hash::sha256_hex(&bytes);
        for row in self.store.list_connector_versions().await? {
            if row.spec.provenance.source_document_sha256.as_deref() == Some(sha.as_str()) && row.connector_id == req.connector_id {
                if let Some(conn) = &req.attach_to_connection {
                    self.attach_connector(conn, &row.spec.reference()).await?;
                }
                return Ok(GenerateConnectorResponse {
                    connector_ref: row.spec.reference(),
                    status: row.status,
                    reused_existing: true,
                    spec: row.spec,
                    manifest: row.manifest,
                    notes: vec!["identical document already registered; reusing existing connector".into()],
                    suspicious_text: vec![],
                    validation_errors: vec![],
                });
            }
        }
        let doc = factory::parse_document(&bytes)?;
        let mut generated = factory::generate_from_openapi(&doc, &req.connector_id, &sha)?;
        // choose next free version if the id exists with different content
        if let Ok(existing) = self.store.get_connector_version(&req.connector_id, None).await {
            let parts: Vec<u64> = existing.version.split('.').filter_map(|p| p.parse().ok()).collect();
            if parts.len() == 3 {
                generated.spec.version = format!("{}.{}.0", parts[0], parts[1] + 1);
            }
        }
        if !generated.validation_errors.is_empty() {
            return Err(MaError::validation(format!("generated spec failed validation: {}", generated.validation_errors.join("; "))));
        }
        self.store.put_connector_version(&generated.spec, &generated.manifest, ConnectorStatus::Draft).await?;
        let reference = generated.spec.reference();
        if let Some(conn) = &req.attach_to_connection {
            self.attach_connector(conn, &reference).await?;
        }
        self.store
            .audit(
                &actor.label(),
                "connector.generated",
                Some(&reference),
                json!({"document_sha256": sha, "resources": generated.spec.resources.len(), "suspicious_passages": generated.suspicious_text.len()}),
            )
            .await?;
        Ok(GenerateConnectorResponse {
            connector_ref: reference,
            status: ConnectorStatus::Draft,
            reused_existing: false,
            spec: generated.spec,
            manifest: generated.manifest,
            notes: generated.notes,
            suspicious_text: generated.suspicious_text,
            validation_errors: generated.validation_errors,
        })
    }

    async fn attach_connector(&self, connection: &str, reference: &str) -> MaResult<()> {
        let mut c = self.store.get_connection(connection).await?;
        if c.kind != "rest" {
            return Err(MaError::validation("generated connectors can only be attached to `rest` connections"));
        }
        c.connector_ref = Some(reference.to_string());
        self.store.put_connection(&c).await
    }

    /// Register a hand-written declarative spec (validated exactly like generated ones).
    pub async fn register_connector_spec(&self, json_text: &str, actor: Actor) -> MaResult<String> {
        let spec = factory::parse_spec(json_text)?;
        let errs = factory::validate_spec(&spec);
        if !errs.is_empty() {
            return Err(MaError::validation(format!("connector spec rejected: {}", errs.join("; "))));
        }
        let mut manifest = CapabilityManifest::new(SystemIdentity {
            kind: "rest".into(),
            name: spec.title.clone(),
            version: spec.api_version.clone(),
            locator: "manual spec".into(),
        });
        manifest.set("read", Evidence::Documented, None);
        self.store.put_connector_version(&spec, &manifest, ConnectorStatus::Draft).await?;
        self.store.audit(&actor.label(), "connector.registered", Some(&spec.reference()), json!({})).await?;
        Ok(spec.reference())
    }

    pub async fn test_connector(&self, connection: &str, actor: Actor) -> MaResult<ConnectorTestResponse> {
        let c = self.store.get_connection(connection).await?;
        let r = c.connector_ref.clone().ok_or_else(|| MaError::config("connection has no generated connector"))?;
        let (id, ver) = r.split_once('@').ok_or_else(|| MaError::config("connector_ref must be id@version"))?;
        let mut row = self.store.get_connector_version(id, Some(ver)).await?;
        let cred = self.credential(c.credential_ref.as_deref()).await?;
        let rest = ma_connector_rest::RestConnector::connect(row.spec.clone(), &c.settings, &c.allowed_hosts, cred).await?;
        let results = factory::run_contract_tests(&rest, &mut row.manifest).await;
        let passed = results.iter().all(|r| r.passed);
        self.store.add_connector_test(id, ver, connection, &results).await?;
        self.store.update_connector_manifest(id, ver, &row.manifest).await?;
        if passed && row.status == ConnectorStatus::Draft {
            self.store.set_connector_status(id, ver, ConnectorStatus::Tested, None).await?;
        }
        self.store.audit(&actor.label(), "connector.tested", Some(&r), json!({"passed": passed})).await?;
        let status = self.store.get_connector_version(id, Some(ver)).await?.status;
        Ok(ConnectorTestResponse { connector_ref: r, passed, status, results, manifest: row.manifest })
    }

    /// Operator-only: activate a tested connector for use in executable plans.
    pub async fn activate_connector(&self, reference: &str, consent: &OperatorConsent) -> MaResult<()> {
        let (id, ver) = reference.split_once('@').ok_or_else(|| MaError::config("use id@version"))?;
        let row = self.store.get_connector_version(id, Some(ver)).await?;
        match self.store.latest_connector_test(id, ver).await? {
            Some((true, _)) => {}
            _ => return Err(MaError::policy("connector must pass contract tests before activation")),
        }
        if row.status == ConnectorStatus::Disabled {
            return Err(MaError::policy("connector is disabled"));
        }
        self.store.set_connector_status(id, ver, ConnectorStatus::Active, Some(consent.operator())).await?;
        self.store
            .audit(&format!("operator:{}", consent.operator()), "connector.activated", Some(reference), json!({"method": consent.method()}))
            .await
    }

    pub async fn connector_registry(&self) -> MaResult<Vec<RegistryEntry>> {
        let mut out = vec![
            RegistryEntry {
                reference: format!("{BUILTIN}-files@{VERSION}"),
                kind: "files".into(),
                title: "CSV / JSON / JSON Lines files".into(),
                status: "builtin".into(),
                generated: false,
                resources: vec![],
            },
            RegistryEntry {
                reference: format!("{BUILTIN}-postgres@{VERSION}"),
                kind: "postgres".into(),
                title: "PostgreSQL".into(),
                status: "builtin".into(),
                generated: false,
                resources: vec![],
            },
        ];
        for row in self.store.list_connector_versions().await? {
            out.push(RegistryEntry {
                reference: row.spec.reference(),
                kind: "rest".into(),
                title: row.spec.title.clone(),
                status: row.status.to_string(),
                generated: true,
                resources: row.spec.resources.iter().map(|r| r.name.clone()).collect(),
            });
        }
        Ok(out)
    }

    // ================================================================= extensions
    pub async fn register_extension(&self, name: &str, module: &[u8], consent: &OperatorConsent) -> MaResult<String> {
        let sb = ma_sandbox::Sandbox::new(Default::default())?;
        sb.load(name, module)?;
        sb.self_test(name)?;
        let sha = ma_types::hash::sha256_hex(module);
        self.store.put_extension(name, &sha, module, "active").await?;
        self.store.audit(&format!("operator:{}", consent.operator()), "extension.activated", Some(name), json!({"sha256": sha})).await?;
        Ok(sha)
    }

    async fn load_extensions(&self, plan: &MigrationPlan) -> MaResult<Option<Arc<dyn ExtensionRunner>>> {
        if plan.extensions.is_empty() {
            return Ok(None);
        }
        let sb = ma_sandbox::Sandbox::new(Default::default())?; // fails closed
        for (name, sha) in &plan.extensions {
            let (stored_sha, module, status) = self.store.get_extension(name).await?;
            let actual = ma_types::hash::sha256_hex(&module);
            if &stored_sha != sha || &actual != sha || status != "active" {
                return Err(MaError::policy(format!("extension `{name}` changed or is not active")));
            }
            sb.load(name, &module)?;
        }
        Ok(Some(Arc::new(sb)))
    }

    // ================================================================= mapping
    pub async fn suggest_mapping(&self, source: &str, destination: &str, pairs: &[(String, String)]) -> MaResult<SuggestionSet> {
        let s = self.discover(source, Actor::Cli).await?;
        let d = self.discover(destination, Actor::Cli).await?;
        let (_, dh, _) = self.connect(destination).await?;
        Ok(suggest_or_copy(&s.schema, &d.schema, dh.connector.creates_objects_on_write(), pairs))
    }

    // ================================================================= plans
    pub async fn create_plan(&self, req: &PlanRequest, actor: Actor) -> MaResult<PlanSummary> {
        let (sc, sh, sref) = self.connect(&req.source).await?;
        let (dc, dh, dref) = self.connect(&req.destination).await?;
        let source_schema = sh.connector.discover_schema().await?;
        let mut destination_schema = dh.connector.discover_schema().await?;
        let mapping = match &req.mapping {
            Some(m) => m.clone(),
            None => suggest_or_copy(&source_schema, &destination_schema, dh.connector.creates_objects_on_write(), &req.pairs).mapping,
        };
        // objects the destination will create on write get their definition from the mapping
        if dh.connector.creates_objects_on_write() {
            destination_schema.objects.retain(|o| !mapping.objects.iter().any(|m| m.target_object == o.name));
        }
        let mut estimates = BTreeMap::new();
        let src = sh.source()?;
        for om in &mapping.objects {
            let n = match source_schema.object(&om.source_object).and_then(|o| o.estimated_count) {
                Some(n) => Some(n),
                None => src.count(&om.source_object).await.ok().flatten(),
            };
            estimates.insert(om.source_object.clone(), n);
        }
        let sink = dh.sink()?;
        let dest_idempotent = mapping
            .objects
            .iter()
            .map(|om| (om.target_object.clone(), sink.idempotent_rewrites(om.write_mode, om.target_key_field.is_some())))
            .collect();
        let mut extensions = BTreeMap::new();
        for om in &mapping.objects {
            for f in &om.fields {
                let mut names = vec![];
                f.expr.extensions(&mut names);
                for n in names {
                    if let Ok((sha, _, status)) = self.store.get_extension(&n).await
                        && status == "active"
                    {
                        extensions.insert(n, sha);
                    }
                }
            }
        }
        let mut destructive = vec![];
        if dc.settings.get("overwrite").and_then(|v| v.as_bool()) == Some(true) {
            destructive.push(Operation::TruncateDestination);
        }
        let input = ma_planning::PlanInput {
            workspace: self.store.workspace().to_string(),
            source: PlanEndpoint {
                connection: sc.name.clone(),
                identity: sh.connector.identify(),
                connector: sref,
                environment: sc.environment,
            },
            destination: PlanEndpoint {
                connection: dc.name.clone(),
                identity: dh.connector.identify(),
                connector: dref.clone(),
                environment: dc.environment,
            },
            source_schema,
            destination_schema,
            mapping,
            options: req.options.clone().unwrap_or_default(),
            estimates,
            dest_creates_objects: dh.connector.creates_objects_on_write(),
            dest_supports_updates: sink.supports_updates(),
            dest_idempotent,
            dest_limitations: dh.connector.capabilities().limitations,
            destructive_operations: destructive,
            extensions,
        };
        let (mut plan, _) = ma_planning::build_plan(input)?;
        if dref.generated {
            let row = self.store.get_connector_version(&dref.id, Some(&dref.version)).await?;
            if row.status != ConnectorStatus::Active {
                plan.risks.push(Risk {
                    severity: Severity::Blocking,
                    code: "connector_not_active".into(),
                    message: format!(
                        "generated connector {}@{} is `{}`; it must be tested and activated by an operator",
                        dref.id, dref.version, row.status
                    ),
                    object: None,
                });
            }
        }
        if plan.source.connector.generated {
            let row = self.store.get_connector_version(&plan.source.connector.id, Some(&plan.source.connector.version)).await?;
            if !matches!(row.status, ConnectorStatus::Active | ConnectorStatus::Tested) {
                plan.risks.push(Risk {
                    severity: Severity::Blocking,
                    code: "connector_not_tested".into(),
                    message: format!("source connector {} is `{}`; run contract tests first", plan.source.connector.id, row.status),
                    object: None,
                });
            }
        }
        let hash = plan.compute_hash();
        let md = ma_planning::render_markdown(&plan, &hash);
        self.store.put_plan(&plan, &hash, &md).await?;
        self.store
            .audit(&actor.label(), "plan.created", Some(&plan.plan_id), json!({"hash": hash, "blocking": plan.has_blocking_risks()}))
            .await?;
        Ok(PlanSummary::from_plan(&plan, &hash, md))
    }

    pub async fn dry_run(&self, plan_id: &str, sample: usize) -> MaResult<DryRunReport> {
        let sp = self.store.get_plan(plan_id).await?;
        let (_, sh, _) = self.connect(&sp.plan.source.connection).await?;
        let ext = self.load_extensions(&sp.plan).await?;
        let report = ma_execution::dryrun::dry_run(&sp.plan, &sp.plan_hash, &sh, ext.as_deref(), sample).await?;
        self.store.put_dry_run(&report, plan_id).await?;
        Ok(report)
    }

    pub async fn preview_plan(&self, plan_id: &str, run_dry_run: bool, sample: usize) -> MaResult<PlanPreview> {
        let sp = self.store.get_plan(plan_id).await?;
        let dry = if run_dry_run { Some(self.dry_run(plan_id, sample).await?) } else { self.store.latest_dry_run(plan_id).await? };
        Ok(PlanPreview {
            summary: PlanSummary::from_plan(&sp.plan, &sp.plan_hash, sp.summary_md.clone()),
            dry_run: dry,
            approval: self.approval_status(plan_id).await?,
            plan: sp.plan,
        })
    }

    pub async fn get_plan(&self, plan_id: &str) -> MaResult<ma_persistence::StoredPlan> {
        self.store.get_plan(plan_id).await
    }

    // ================================================================= approvals
    /// Agents may *request* approval. This never grants it.
    pub async fn request_approval(&self, plan_id: &str, note: Option<&str>, actor: Actor) -> MaResult<ApprovalStatus> {
        let sp = self.store.get_plan(plan_id).await?;
        self.store.request_approval(plan_id, &sp.plan_hash, &actor.label(), note).await?;
        self.store.audit(&actor.label(), "approval.requested", Some(plan_id), json!({"hash": sp.plan_hash})).await?;
        self.approval_status(plan_id).await
    }

    pub async fn approval_status(&self, plan_id: &str) -> MaResult<ApprovalStatus> {
        let sp = self.store.get_plan(plan_id).await?;
        let cmd = format!("migrate-anything plan approve {plan_id}");
        let approvals = self.store.approvals_for_plan(plan_id).await?;
        let mut state = ApprovalState::NotRequested;
        let mut detail = "no approval yet".to_string();
        for a in &approvals {
            match ma_security::approval::verify(&self.approval_key, a, &sp.plan, &sp.plan_hash, &[]) {
                Ok(()) => {
                    state = ApprovalState::Approved;
                    detail = format!(
                        "approved by {} ({}) until {}",
                        a.claims.operator,
                        a.claims.verification_method,
                        a.claims.expires_at.to_rfc3339()
                    );
                    break;
                }
                Err(e) => {
                    state = if a.revoked {
                        ApprovalState::Revoked
                    } else if e.message.contains("expired") {
                        ApprovalState::Expired
                    } else {
                        ApprovalState::Invalid
                    };
                    detail = e.message.clone();
                }
            }
        }
        if matches!(state, ApprovalState::NotRequested) && self.store.has_approval_request(plan_id, &sp.plan_hash).await? {
            state = ApprovalState::Pending;
            detail = "waiting for an operator to approve on a terminal".into();
        }
        Ok(ApprovalStatus { plan_id: plan_id.into(), plan_hash: sp.plan_hash, state, detail, operator_command: cmd })
    }

    /// Operator-only: create a signed approval bound to this exact plan.
    pub async fn approve_plan(
        &self,
        plan_id: &str,
        consent: &OperatorConsent,
        ttl: Duration,
        allow_destructive: bool,
    ) -> MaResult<ApprovalRecord> {
        let sp = self.store.get_plan(plan_id).await?;
        if sp.plan.compute_hash() != sp.plan_hash {
            return Err(MaError::policy("stored plan does not match its hash; refusing to approve"));
        }
        if sp.plan.has_blocking_risks() {
            let b: Vec<String> = sp.plan.risks.iter().filter(|r| r.severity == Severity::Blocking).map(|r| r.message.clone()).collect();
            return Err(MaError::policy(format!("plan has blocking issues and cannot be approved: {}", b.join("; "))));
        }
        if sp.plan.operations.iter().any(|o| o.is_destructive()) && !allow_destructive {
            return Err(MaError::policy("plan contains destructive operations; approve with --allow-destructive"));
        }
        let now = chrono::Utc::now();
        let claims = ApprovalClaims {
            approval_id: format!("appr-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            workspace: sp.plan.workspace.clone(),
            plan_id: plan_id.into(),
            plan_hash: sp.plan_hash.clone(),
            source_identity: sp.plan.source.identity.clone(),
            destination_identity: sp.plan.destination.identity.clone(),
            source_connector: sp.plan.source.connector.clone(),
            destination_connector: sp.plan.destination.connector.clone(),
            permitted_operations: sp.plan.operations.clone(),
            destructive_authorized: allow_destructive,
            operator: consent.operator().to_string(),
            verification_method: consent.method().to_string(),
            target_environment: sp.plan.destination.environment,
            issued_at: now,
            expires_at: now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::hours(24)),
        };
        let signature = ma_security::approval::sign(&self.approval_key, &claims);
        let rec = ApprovalRecord { claims, signature, revoked: false };
        self.store.put_approval(&rec).await?;
        self.store
            .audit(
                &format!("operator:{}", consent.operator()),
                "plan.approved",
                Some(plan_id),
                json!({"approval_id": rec.claims.approval_id, "hash": sp.plan_hash, "method": consent.method()}),
            )
            .await?;
        Ok(rec)
    }

    async fn valid_approval(&self, plan: &MigrationPlan, hash: &str) -> MaResult<ApprovalRecord> {
        let approvals = self.store.approvals_for_plan(&plan.plan_id).await?;
        let mut last_err = MaError::policy(format!(
            "plan {} has no operator approval; writes are refused. An operator must run `migrate-anything plan approve {}`",
            plan.plan_id, plan.plan_id
        ));
        // approvals are ordered newest first; report the newest approval's rejection reason
        let mut first_err = None;
        for a in approvals {
            match ma_security::approval::verify(&self.approval_key, &a, plan, hash, &plan.operations) {
                Ok(()) => return Ok(a),
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        if let Some(e) = first_err {
            last_err = e;
        }
        Err(last_err)
    }

    /// Connect a plan endpoint and prove it is still the system the plan (and its approval) was
    /// made for. A connection that was re-pointed, re-typed or given another connector after
    /// planning is refused: the approval is bound to systems, not to connection names.
    async fn connect_endpoint(&self, ep: &PlanEndpoint) -> MaResult<ConnectorHandle> {
        let (cfg, handle, cref) = self.connect(&ep.connection).await?;
        let live = handle.connector.identify();
        let same_system = live.kind == ep.identity.kind && live.name == ep.identity.name && live.locator == ep.identity.locator;
        if !same_system || cref != ep.connector || cfg.environment != ep.environment {
            return Err(MaError::policy(format!(
                "connection `{}` no longer matches the approved plan (planned: {} at {} [{}], connector {}@{}; now: {} at {} [{}], connector {}@{}). Create and approve a new plan.",
                ep.connection,
                ep.identity.name,
                ep.identity.locator,
                ep.environment,
                ep.connector.id,
                ep.connector.version,
                live.name,
                live.locator,
                cfg.environment,
                cref.id,
                cref.version
            )));
        }
        Ok(handle)
    }

    async fn ensure_connectors_active(&self, plan: &MigrationPlan) -> MaResult<()> {
        for (ep, need_active) in [(&plan.source, false), (&plan.destination, true)] {
            if ep.connector.generated {
                let row = self.store.get_connector_version(&ep.connector.id, Some(&ep.connector.version)).await?;
                let ok = row.status == ConnectorStatus::Active || (!need_active && row.status == ConnectorStatus::Tested);
                if !ok {
                    return Err(MaError::policy(format!(
                        "generated connector {}@{} is `{}`",
                        ep.connector.id, ep.connector.version, row.status
                    )));
                }
            }
        }
        Ok(())
    }

    // ================================================================= jobs
    pub async fn start_job(&self, plan_id: &str, spawn: SpawnMode, actor: Actor) -> MaResult<JobInfo> {
        let sp = self.store.get_plan(plan_id).await?;
        let approval = match self.valid_approval(&sp.plan, &sp.plan_hash).await {
            Ok(a) => a,
            Err(e) => {
                self.store.audit(&actor.label(), "job.rejected", Some(plan_id), json!({"reason": e.message})).await?;
                return Err(e);
            }
        };
        self.ensure_connectors_active(&sp.plan).await?;
        self.connect_endpoint(&sp.plan.source).await?;
        self.connect_endpoint(&sp.plan.destination).await?;
        let job_id = format!("job-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        self.store.create_job(&job_id, &sp.plan, &sp.plan_hash, &approval.claims.approval_id).await?;
        self.store
            .audit(&actor.label(), "job.created", Some(&job_id), json!({"plan": plan_id, "approval": approval.claims.approval_id}))
            .await?;
        self.launch(&job_id, spawn).await?;
        self.store.get_job(&job_id).await
    }

    async fn launch(&self, job_id: &str, spawn: SpawnMode) -> MaResult<()> {
        match spawn {
            SpawnMode::Detached => {
                let exe = std::env::var("MA_WORKER_EXE").map(PathBuf::from).or_else(|_| std::env::current_exe())?;
                let log =
                    std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("logs").join(format!("{job_id}.log")))?;
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("--home")
                    .arg(&self.home)
                    .args(["worker", "run", "--job", job_id])
                    .stdin(std::process::Stdio::null())
                    .stdout(log.try_clone()?)
                    .stderr(log);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: survives the parent (MCP client)
                    cmd.creation_flags(0x0000_0008 | 0x0000_0200);
                }
                let child = cmd.spawn().map_err(|e| MaError::internal(format!("could not start worker: {e}")))?;
                self.store.audit(&Actor::Worker.label(), "job.worker_spawned", Some(job_id), json!({"pid": child.id()})).await?;
                Ok(())
            }
            SpawnMode::Inline => Ok(()),
        }
    }

    /// Execute a job in this process (used by `worker run` and tests).
    ///
    /// A failure before the job actually starts (expired approval, a connection that no longer
    /// matches the plan, an unreachable system) is recorded on the job, so `jobs status`, the UI
    /// and MCP clients show why nothing is happening instead of a job that stays "pending".
    pub async fn run_worker(&self, job_id: &str) -> MaResult<RunEnd> {
        let runner = match self.prepare_runner(job_id).await {
            Ok(r) => r,
            Err(e) => {
                self.record_start_failure(job_id, &e).await;
                return Err(e);
            }
        };
        runner.run().await
    }

    async fn record_start_failure(&self, job_id: &str, e: &MaError) {
        let Ok(job) = self.store.get_job(job_id).await else { return };
        if job.status.is_terminal() {
            return;
        }
        // never touch a job that another live worker is running
        let live = matches!(self.store.lease_state(job_id).await, Ok((Some(_), Some(exp))) if exp > chrono::Utc::now().timestamp_millis());
        if live {
            return;
        }
        let msg = format!("the worker could not start: {}", e.message);
        let _ = self.store.set_job_status(job_id, JobStatus::Failed, Some(&msg)).await;
        let _ = self.store.audit(&Actor::Worker.label(), "job.start_failed", Some(job_id), json!({"error": e.message})).await;
    }

    async fn prepare_runner(&self, job_id: &str) -> MaResult<JobRunner> {
        let job = self.store.get_job(job_id).await?;
        let sp = self.store.get_plan(&job.plan_id).await?;
        if sp.plan_hash != job.plan_hash || sp.plan.compute_hash() != job.plan_hash {
            return Err(MaError::policy("plan changed after the job was created"));
        }
        let approval = self.store.get_approval(&self.store.job_approval_id(job_id).await?).await?;
        ma_security::approval::verify(&self.approval_key, &approval, &sp.plan, &sp.plan_hash, &sp.plan.operations)?;
        self.ensure_connectors_active(&sp.plan).await?;
        let source = self.connect_endpoint(&sp.plan.source).await?;
        let destination = self.connect_endpoint(&sp.plan.destination).await?;
        let lease_secs = std::env::var("MA_LEASE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(30u64);
        let runner = JobRunner {
            store: self.store.clone(),
            extensions: self.load_extensions(&sp.plan).await?,
            plan: sp.plan,
            plan_hash: sp.plan_hash,
            source,
            destination,
            job_id: job_id.into(),
            owner: format!("worker-{}-{}", std::process::id(), ma_security::random_hex(4)),
            lease: Duration::from_secs(lease_secs.max(1)),
            lease_lost: Default::default(),
        };
        Ok(runner)
    }

    pub async fn job_status(&self, job_id: &str) -> MaResult<JobInfo> {
        self.store.get_job(job_id).await
    }

    pub async fn list_jobs(&self) -> MaResult<Vec<(String, String, String, String)>> {
        self.store.list_jobs().await
    }

    pub async fn pause_job(&self, job_id: &str, actor: Actor) -> MaResult<JobInfo> {
        let j = self.store.get_job(job_id).await?;
        if j.status.is_terminal() {
            return Err(MaError::config(format!("job is already {}", j.status.as_str())));
        }
        self.store.set_job_control(job_id, JobControl::Pause).await?;
        if j.status == JobStatus::Pending {
            self.store.set_job_status(job_id, JobStatus::Paused, None).await?;
        }
        self.store.audit(&actor.label(), "job.pause_requested", Some(job_id), json!({})).await?;
        self.store.get_job(job_id).await
    }

    pub async fn cancel_job(&self, job_id: &str, actor: Actor) -> MaResult<JobInfo> {
        let j = self.store.get_job(job_id).await?;
        if j.status.is_terminal() {
            return Err(MaError::config(format!("job is already {}", j.status.as_str())));
        }
        self.store.set_job_control(job_id, JobControl::Cancel).await?;
        let (owner, expires) = self.store.lease_state(job_id).await?;
        let live = owner.is_some() && expires.is_some_and(|e| e > chrono::Utc::now().timestamp_millis());
        if !live {
            self.store.set_job_status(job_id, JobStatus::Cancelled, None).await?;
        }
        self.store.audit(&actor.label(), "job.cancel_requested", Some(job_id), json!({})).await?;
        self.store.get_job(job_id).await
    }

    /// Resume a paused, failed or crashed job from its last checkpoint.
    pub async fn resume_job(&self, job_id: &str, spawn: SpawnMode, actor: Actor) -> MaResult<JobInfo> {
        let j = self.store.get_job(job_id).await?;
        if matches!(j.status, JobStatus::Completed | JobStatus::Cancelled) {
            return Err(MaError::config(format!("job is {}; nothing to resume", j.status.as_str())));
        }
        let (owner, expires) = self.store.lease_state(job_id).await?;
        if owner.is_some() && expires.is_some_and(|e| e > chrono::Utc::now().timestamp_millis()) && j.status == JobStatus::Running {
            return Err(MaError::config("job is running under a live worker lease"));
        }
        self.store.set_job_control(job_id, JobControl::None).await?;
        self.store.set_job_status(job_id, JobStatus::Pending, None).await?;
        self.store.audit(&actor.label(), "job.resume_requested", Some(job_id), json!({})).await?;
        self.launch(job_id, spawn).await?;
        self.store.get_job(job_id).await
    }

    pub async fn verify_job(&self, job_id: &str, actor: Actor) -> MaResult<VerificationResult> {
        let job = self.store.get_job(job_id).await?;
        let sp = self.store.get_plan(&job.plan_id).await?;
        let source = self.connect_endpoint(&sp.plan.source).await?;
        let destination = self.connect_endpoint(&sp.plan.destination).await?;
        let v = ma_validation::Verifier {
            store: &self.store,
            plan: &sp.plan,
            source: &source,
            destination: &destination,
            extensions: self.load_extensions(&sp.plan).await?,
            job_id,
        }
        .verify()
        .await?;
        self.store.audit(&actor.label(), "job.verified", Some(job_id), json!({"discrepancies": v.discrepancies.len()})).await?;
        Ok(v)
    }

    pub async fn report(&self, job_id: &str) -> MaResult<MigrationReport> {
        let job = self.store.get_job(job_id).await?;
        let sp = self.store.get_plan(&job.plan_id).await?;
        ma_validation::build_report(&self.store, &sp.plan, &sp.plan_hash, job_id).await
    }

    pub async fn exceptions(&self, job_id: &str, limit: i64) -> MaResult<Vec<ma_persistence::ExceptionRow>> {
        self.store.exceptions(job_id, limit).await
    }
}

/// Heuristic suggestion; for destinations that create objects on write and have no matching
/// objects yet (e.g. an empty folder), propose a 1:1 copy keyed on the source primary key.
pub fn suggest_or_copy(src: &SystemSchema, dst: &SystemSchema, dest_creates: bool, pairs: &[(String, String)]) -> SuggestionSet {
    if !dest_creates || !dst.objects.is_empty() {
        return ma_mapping::suggest_mapping(src, dst, pairs, &SuggestOptions::default());
    }
    let objects = src
        .objects
        .iter()
        .filter(|o| o.readable)
        .map(|o| {
            let key = o.primary_key.first().cloned();
            ObjectMapping {
                source_object: o.name.clone(),
                target_object: o.name.clone(),
                target_key_field: key.clone(),
                fields: o
                    .fields
                    .iter()
                    .map(|f| {
                        let mut fm = FieldMapping::accepted(f.name.clone(), Expr::field(f.name.clone()));
                        fm.explanation = "copied 1:1 into a new destination object".into();
                        fm.evidence = vec!["destination creates objects on write".into()];
                        fm
                    })
                    .collect(),
                unmapped_fields: UnmappedFieldPolicy::Archive,
                skip_fields: vec![],
                write_mode: if key.is_some() { WriteMode::Upsert } else { WriteMode::Insert },
            }
        })
        .collect::<Vec<_>>();
    SuggestionSet {
        pairings: objects
            .iter()
            .map(|o| ma_types::ObjectPairing {
                source_object: o.source_object.clone(),
                target_object: o.target_object.clone(),
                confidence: 1.0,
                explanation: "new destination object".into(),
            })
            .collect(),
        suggestions: vec![],
        mapping: MappingSpec { mapping_version: 1, objects },
        unmatched_source_fields: Default::default(),
        unmatched_required_targets: Default::default(),
        needs_review: 0,
    }
}
