//! JSON API handlers. Each one validates input, calls one or two `ma_core::App` methods and
//! returns JSON. The caller is the operator in their own browser (see the guard in `lib.rs`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use ma_core::{Actor, DocumentSource, GenerateConnectorRequest, PlanRequest, SpawnMode};
use ma_types::{ConnectionConfig, Environment, ExecutionOptions, MaError, MappingSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{ApiError, ApiResult, PendingChallenge, UiState, ok};

type St = State<Arc<UiState>>;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(MaError::validation(msg))
}

// ------------------------------------------------------------------------------- overview

pub async fn state(State(st): St) -> ApiResult {
    let app = &st.app;
    let credentials: Vec<String> = app.store.list_credentials().await?;
    let connections: Vec<Value> = app
        .store
        .list_connections()
        .await?
        .into_iter()
        .map(|c| {
            json!({
                "name": c.name, "kind": c.kind, "environment": c.environment, "settings": c.settings,
                "has_credential": c.credential_ref.as_ref().is_some_and(|r| credentials.contains(r)),
                "connector_ref": c.connector_ref, "allowed_hosts": c.allowed_hosts,
            })
        })
        .collect();
    let jobs: Vec<Value> = app
        .list_jobs()
        .await?
        .into_iter()
        .take(25)
        .map(|(job_id, plan_id, status, created_at)| json!({"job_id": job_id, "plan_id": plan_id, "status": status, "created_at": created_at}))
        .collect();
    let connectors = app.connector_registry().await?;
    ok(json!({
        "version": ma_core::VERSION,
        "home": app.home().display().to_string(),
        "operator": ma_security::os_operator(),
        "approvals": {
            "enabled": st.session.is_some(),
            "how": if st.session.is_some() {
                "A one-time code is shown in the terminal window where Migrate Anything is running."
            } else {
                "This Studio was not started from an interactive terminal, so it cannot approve. Use the command shown instead."
            },
        },
        "connections": connections,
        "connectors": connectors,
        "jobs": jobs,
    }))
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    path: String,
}

/// Folder picker support: sub-folders and a count of data files in a folder.
pub async fn browse(Query(q): Query<BrowseQuery>) -> ApiResult {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    let path = if q.path.trim().is_empty() { std::path::PathBuf::from(home) } else { std::path::PathBuf::from(q.path.trim()) };
    let path = std::path::absolute(&path).map_err(|e| bad(format!("invalid path: {e}")))?;
    if !path.is_dir() {
        return Err(bad(format!("{} is not a folder", path.display())));
    }
    let mut dirs = vec![];
    let mut files: BTreeMap<String, usize> = BTreeMap::new();
    let entries = std::fs::read_dir(&path).map_err(|e| bad(format!("cannot read {}: {e}", path.display())))?;
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !name.starts_with('.') && !name.starts_with('$') {
                dirs.push(json!({"name": name, "path": p.display().to_string()}));
            }
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if matches!(ext.as_str(), "csv" | "json" | "jsonl") {
                *files.entry(ext).or_default() += 1;
            }
        }
    }
    dirs.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
    let mut roots = vec![];
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            if std::path::Path::new(&root).is_dir() {
                roots.push(root);
            }
        }
    } else {
        roots.push("/".to_string());
    }
    ok(json!({
        "path": path.display().to_string(),
        "parent": path.parent().map(|p| p.display().to_string()),
        "dirs": dirs, "data_files": files, "roots": roots,
    }))
}

pub async fn demo(State(st): St) -> ApiResult {
    ok(crate::demo::setup(&st.app).await?)
}

// ------------------------------------------------------------------------------- connections

#[derive(Deserialize)]
pub struct SaveConnection {
    name: String,
    kind: String,
    #[serde(default)]
    environment: Option<String>,
    settings: Value,
    /// Password / API key. Stored encrypted; never returned.
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
}

pub async fn save_connection(State(st): St, Json(b): Json<SaveConnection>) -> ApiResult {
    let app = &st.app;
    let name = b.name.trim().to_string();
    let existing = app.store.get_connection(&name).await.ok();
    let environment: Environment = b.environment.as_deref().unwrap_or("production").parse().map_err(|e: String| bad(e))?;
    let mut credential_ref = existing.as_ref().and_then(|c| c.credential_ref.clone());
    if let Some(secret) = b.secret.as_deref().filter(|s| !s.is_empty()) {
        let cred = format!("{name}-credential");
        app.set_credential(&cred, secret).await?;
        credential_ref = Some(cred);
    }
    let mut allowed_hosts = b.allowed_hosts;
    if b.kind == "rest" && allowed_hosts.is_empty() {
        // the operator typed this base URL in their own browser: allow exactly its host
        if let Some(host) = b
            .settings
            .get("base_url")
            .and_then(|u| u.as_str())
            .and_then(|u| url::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(String::from))
        {
            allowed_hosts.push(host.trim_start_matches('[').trim_end_matches(']').to_string());
        }
    }
    // keep the connector only while the connection still points at the same API
    let same_target = existing.as_ref().is_some_and(|c| c.kind == b.kind && c.settings.get("base_url") == b.settings.get("base_url"));
    let cfg = ConnectionConfig {
        name: name.clone(),
        kind: b.kind,
        environment,
        settings: b.settings,
        credential_ref,
        allowed_hosts,
        connector_ref: if same_target { existing.and_then(|c| c.connector_ref) } else { None },
    };
    app.add_connection(cfg, Actor::Ui).await?;
    ok(json!({ "saved": name }))
}

pub async fn discover(State(st): St, Path(name): Path<String>) -> ApiResult {
    ok(st.app.discover(&name, Actor::Ui).await?)
}

#[derive(Deserialize)]
pub struct ConnectorBody {
    #[serde(default)]
    connector_id: Option<String>,
    #[serde(default)]
    openapi_url: Option<String>,
    #[serde(default)]
    openapi_file: Option<String>,
}

/// Generate a connector from an OpenAPI document, attach it and run the read-only contract tests.
pub async fn generate_connector(State(st): St, Path(name): Path<String>, Json(b): Json<ConnectorBody>) -> ApiResult {
    let source = match (b.openapi_url.filter(|s| !s.trim().is_empty()), b.openapi_file.filter(|s| !s.trim().is_empty())) {
        (Some(url), None) => DocumentSource::Url { url: url.trim().to_string(), connection: name.clone() },
        (None, Some(path)) => DocumentSource::File { path: path.trim().to_string() },
        _ => return Err(bad("give either the web address of the API description (OpenAPI) or a file path, not both")),
    };
    let connector_id = b.connector_id.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        let id: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        format!("{}-api", id.trim_matches('-'))
    });
    let generated = st
        .app
        .generate_connector(&GenerateConnectorRequest { source, connector_id, attach_to_connection: Some(name.clone()) }, Actor::Ui)
        .await?;
    let (test, test_error) = match st.app.test_connector(&name, Actor::Ui).await {
        Ok(t) => (Some(t), None),
        Err(e) => (None, Some(e.message)),
    };
    ok(json!({ "generated": generated, "test": test, "test_error": test_error }))
}

#[derive(Deserialize)]
pub struct SampleBody {
    object: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// A few example records, shown next to field names while mapping. Stays on this machine.
pub async fn sample(State(st): St, Path(name): Path<String>, Json(b): Json<SampleBody>) -> ApiResult {
    let (_, handle, _) = st.app.connect(&name).await?;
    let batch = handle.source()?.read_batch(&b.object, None, b.limit.unwrap_or(5).clamp(1, 20)).await?;
    let records: Vec<Value> = batch.records.iter().map(|r| r.to_plain_json()).collect();
    ok(json!({ "object": b.object, "records": records }))
}

#[derive(Deserialize)]
pub struct ActivateBody {
    reference: String,
    challenge_id: String,
    code: String,
}

pub async fn activate_connector(State(st): St, Json(b): Json<ActivateBody>) -> ApiResult {
    let consent = confirm(&st, &b.challenge_id, &b.code, &format!("connector:{}", b.reference))?;
    st.app.activate_connector(&b.reference, &consent).await?;
    ok(json!({ "activated": b.reference }))
}

// ------------------------------------------------------------------------------- mapping & plans

#[derive(Deserialize)]
pub struct SuggestBody {
    source: String,
    destination: String,
}

pub async fn suggest(State(st): St, Json(b): Json<SuggestBody>) -> ApiResult {
    let app = &st.app;
    let src = app.discover(&b.source, Actor::Ui).await?;
    let dst = app.discover(&b.destination, Actor::Ui).await?;
    let (_, dest_handle, _) = app.connect(&b.destination).await?;
    let creates = dest_handle.connector.creates_objects_on_write();
    let supports_updates = dest_handle.sink.as_ref().is_some_and(|s| s.supports_updates());
    let suggestion = ma_core::suggest_or_copy(&src.schema, &dst.schema, creates, &[]);
    ok(json!({
        "source": src, "destination": dst,
        "destination_creates_objects": creates,
        "destination_supports_updates": supports_updates,
        "suggestion": suggestion,
    }))
}

#[derive(Deserialize)]
pub struct PlanBody {
    source: String,
    destination: String,
    #[serde(default)]
    mapping: Option<MappingSpec>,
    #[serde(default)]
    batch_size: Option<usize>,
}

pub async fn create_plan(State(st): St, Json(b): Json<PlanBody>) -> ApiResult {
    let options = b.batch_size.map(|n| ExecutionOptions { batch_size: n.clamp(1, 50_000), ..Default::default() });
    let req = PlanRequest { source: b.source, destination: b.destination, mapping: b.mapping, pairs: vec![], options };
    ok(st.app.create_plan(&req, Actor::Ui).await?)
}

pub async fn get_plan(State(st): St, Path(id): Path<String>) -> ApiResult {
    let p = st.app.preview_plan(&id, false, 0).await?;
    ok(json!({ "summary": p.summary, "dry_run": p.dry_run, "approval": p.approval, "plan": p.plan }))
}

#[derive(Deserialize, Default)]
pub struct DryRunBody {
    #[serde(default)]
    sample: Option<usize>,
}

pub async fn dry_run(State(st): St, Path(id): Path<String>, body: Option<Json<DryRunBody>>) -> ApiResult {
    let sample = body.and_then(|b| b.0.sample).unwrap_or(200).clamp(1, 5000);
    ok(st.app.dry_run(&id, sample).await?)
}

// ------------------------------------------------------------------------------- approvals

#[derive(Deserialize)]
pub struct ChallengeBody {
    /// `plan` or `connector`
    kind: String,
    /// plan id or connector reference
    subject: String,
}

/// Print a one-time code on the terminal. The operator types it in the browser to approve.
pub async fn challenge(State(st): St, Json(b): Json<ChallengeBody>) -> ApiResult {
    let Some(session) = &st.session else {
        return Err(ApiError(MaError::policy(
            "approvals are disabled because Studio was not started from an interactive terminal; approve with the command line instead",
        )));
    };
    let what = match b.kind.as_str() {
        "plan" => {
            let sp = st.app.get_plan(&b.subject).await?;
            format!(
                "Approve migration plan {} ({} -> {}, sha256 {})",
                b.subject,
                sp.plan.source.connection,
                sp.plan.destination.connection,
                &sp.plan_hash[..12]
            )
        }
        "connector" => format!("Activate generated connector {}", b.subject),
        other => return Err(bad(format!("unknown approval kind `{other}`"))),
    };
    let id = ma_security::random_hex(8);
    let challenge = session.challenge(&what);
    let mut map = st.challenges.lock().expect("lock");
    map.retain(|_, c| !c.challenge.is_expired());
    map.insert(id.clone(), PendingChallenge { challenge, purpose: format!("{}:{}", b.kind, b.subject) });
    ok(json!({ "challenge_id": id, "valid_seconds": 300 }))
}

fn confirm(st: &UiState, challenge_id: &str, code: &str, purpose: &str) -> Result<ma_core::OperatorConsent, ApiError> {
    let mut map = st.challenges.lock().expect("lock");
    let pending = map.get_mut(challenge_id).ok_or_else(|| ApiError(MaError::policy("no approval code is pending; request a new one")))?;
    if pending.purpose != purpose {
        return Err(ApiError(MaError::policy("this approval code was issued for something else")));
    }
    let consent = pending.challenge.confirm(code)?;
    map.remove(challenge_id);
    Ok(consent)
}

#[derive(Deserialize)]
pub struct ApproveBody {
    challenge_id: String,
    code: String,
    #[serde(default)]
    allow_destructive: bool,
}

pub async fn approve_plan(State(st): St, Path(id): Path<String>, Json(b): Json<ApproveBody>) -> ApiResult {
    let consent = confirm(&st, &b.challenge_id, &b.code, &format!("plan:{id}"))?;
    st.app.approve_plan(&id, &consent, Duration::from_secs(24 * 3600), b.allow_destructive).await?;
    ok(st.app.approval_status(&id).await?)
}

// ------------------------------------------------------------------------------- jobs

#[derive(Deserialize)]
pub struct StartBody {
    plan_id: String,
}

pub async fn start_job(State(st): St, Json(b): Json<StartBody>) -> ApiResult {
    ok(st.app.start_job(&b.plan_id, SpawnMode::Detached, Actor::Ui).await?)
}

pub async fn get_job(State(st): St, Path(id): Path<String>) -> ApiResult {
    ok(st.app.job_status(&id).await?)
}

pub async fn job_action(State(st): St, Path((id, action)): Path<(String, String)>) -> ApiResult {
    let app = &st.app;
    match action.as_str() {
        "pause" => ok(app.pause_job(&id, Actor::Ui).await?),
        "cancel" => ok(app.cancel_job(&id, Actor::Ui).await?),
        "resume" => ok(app.resume_job(&id, SpawnMode::Detached, Actor::Ui).await?),
        "verify" => ok(app.verify_job(&id, Actor::Ui).await?),
        other => Err(bad(format!("unknown job action `{other}`"))),
    }
}

pub async fn report(State(st): St, Path(id): Path<String>) -> ApiResult {
    let r = st.app.report(&id).await?;
    ok(json!({ "text": r.to_text(), "report": r }))
}

/// The stored result of the last verification (does not run it again).
pub async fn verification(State(st): St, Path(id): Path<String>) -> ApiResult {
    match st.app.store.get_validation(&id).await? {
        Some(v) => ok(v),
        None => Err(ApiError(MaError::not_found("this job has not been verified yet"))),
    }
}

pub async fn exceptions(State(st): St, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let limit = q.get("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(200).clamp(1, 2000);
    ok(st.app.exceptions(&id, limit).await?)
}
