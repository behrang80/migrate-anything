//! Migrate Anything Studio — the local web interface.
//!
//! A thin HTTP adapter over `ma_core::App` (the same service layer the CLI and the MCP server
//! use) plus a static single-page app embedded in the binary. No business logic lives here.
//!
//! Security model:
//! * binds to 127.0.0.1 only;
//! * every API call needs the session cookie, which is set from the one-time token in the URL
//!   printed on the terminal that started the server;
//! * the `Host` header must be this server (blocks DNS rebinding) and state-changing requests
//!   must come from this origin;
//! * approvals need an [`InteractiveSession`]: the server prints a code on the terminal and the
//!   operator types it into the browser. Without a terminal, approvals are disabled.

mod api;
mod demo;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ma_core::{App, Challenge, InteractiveSession};
use ma_types::{ErrorClass, MaError};
use serde_json::json;

const INDEX_HTML: &str = include_str!("../static/index.html");
const LOCKED_HTML: &str = include_str!("../static/locked.html");
const APP_CSS: &str = include_str!("../static/app.css");
const APP_JS: &str = include_str!("../static/app.js");
const RAIN_JS: &str = include_str!("../static/rain.js");
const FAVICON_SVG: &str = include_str!("../static/favicon.svg");

const COOKIE: &str = "ma_session";
/// Preferred port; if it is taken a free one is chosen.
pub const DEFAULT_PORT: u16 = 4747;

pub struct UiOptions {
    /// 0 = use [`DEFAULT_PORT`] when free, otherwise any free port.
    pub port: u16,
    /// Present only when the server was started from an interactive terminal.
    pub session: Option<InteractiveSession>,
}

pub(crate) struct PendingChallenge {
    pub challenge: Challenge,
    /// What the code approves, e.g. `plan:plan-123`. A code cannot be reused for something else.
    pub purpose: String,
}

pub(crate) struct UiState {
    pub app: Arc<App>,
    pub token: String,
    pub port: u16,
    pub session: Option<InteractiveSession>,
    pub challenges: Mutex<HashMap<String, PendingChallenge>>,
}

pub struct RunningUi {
    pub port: u16,
    /// Open this in a browser: it carries the one-time session token.
    pub url: String,
    state: Arc<UiState>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl RunningUi {
    /// Serve until the process is stopped.
    pub async fn wait(self) -> std::io::Result<()> {
        self.task.await.unwrap_or(Ok(()))
    }

    pub fn approvals_enabled(&self) -> bool {
        self.state.session.is_some()
    }

    pub fn stop(self) {
        self.task.abort();
    }

    /// Test-only: the code that was "printed on the terminal" for a challenge.
    #[cfg(feature = "test-approval")]
    pub fn challenge_code(&self, challenge_id: &str) -> Option<String> {
        self.state.challenges.lock().expect("lock").get(challenge_id).map(|c| c.challenge.code_for_tests().to_string())
    }

    /// Test-only: the session token (tests set the cookie themselves).
    #[cfg(feature = "test-approval")]
    pub fn token(&self) -> &str {
        &self.state.token
    }
}

/// Start the UI server on 127.0.0.1.
pub async fn serve(app: Arc<App>, opts: UiOptions) -> std::io::Result<RunningUi> {
    let listener = if opts.port == 0 {
        match tokio::net::TcpListener::bind(("127.0.0.1", DEFAULT_PORT)).await {
            Ok(l) => l,
            Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?,
        }
    } else {
        tokio::net::TcpListener::bind(("127.0.0.1", opts.port)).await?
    };
    let port = listener.local_addr()?.port();
    let token = ma_security::random_hex(24);
    let state = Arc::new(UiState { app, token: token.clone(), port, session: opts.session, challenges: Mutex::new(HashMap::new()) });

    let api = Router::new()
        .route("/state", get(api::state))
        .route("/fs", get(api::browse))
        .route("/demo", post(api::demo))
        .route("/connections", post(api::save_connection))
        .route("/connections/{name}/discover", post(api::discover))
        .route("/connections/{name}/connector", post(api::generate_connector))
        .route("/connections/{name}/sample", post(api::sample))
        .route("/connectors/activate", post(api::activate_connector))
        .route("/mapping/suggest", post(api::suggest))
        .route("/plans", post(api::create_plan))
        .route("/plans/{id}", get(api::get_plan))
        .route("/plans/{id}/dry-run", post(api::dry_run))
        .route("/plans/{id}/approve", post(api::approve_plan))
        .route("/challenges", post(api::challenge))
        .route("/jobs", post(api::start_job))
        .route("/jobs/{id}", get(api::get_job))
        .route("/jobs/{id}/{action}", post(api::job_action))
        .route("/jobs/{id}/report", get(api::report))
        .route("/jobs/{id}/verification", get(api::verification))
        .route("/jobs/{id}/exceptions", get(api::exceptions));

    let router = Router::new()
        .route("/", get(index))
        .route("/assets/{name}", get(asset))
        .route("/favicon.svg", get(|| async { asset_response("image/svg+xml", FAVICON_SVG) }))
        .nest("/api", api)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state.clone());

    let task = tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await });
    Ok(RunningUi { port, url: format!("http://127.0.0.1:{port}/?t={token}"), state, task })
}

/// Open a URL in the operator's default browser. Failure is not fatal: the URL is also printed.
pub fn open_in_browser(url: &str) -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok()
}

// ---------------------------------------------------------------------------- guard & statics

fn has_session(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .any(|(k, v)| k == COOKIE && v == token)
}

async fn guard(State(st): State<Arc<UiState>>, req: Request, next: Next) -> Response {
    let origins = [format!("127.0.0.1:{}", st.port), format!("localhost:{}", st.port)];
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    if !origins.iter().any(|o| o == host) {
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    if req.uri().path().starts_with("/api/") {
        if !has_session(req.headers(), &st.token) {
            return ApiError(MaError::policy("this browser session is not signed in; open the link printed in the terminal"))
                .into_response();
        }
        if req.method() != axum::http::Method::GET {
            let origin = req.headers().get(header::ORIGIN).and_then(|h| h.to_str().ok());
            let same = origin.map(|o| origins.iter().any(|x| o == format!("http://{x}"))).unwrap_or(false);
            if !same {
                return ApiError(MaError::policy("cross-origin request refused")).into_response();
            }
        }
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        "content-security-policy",
        "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
            .parse()
            .expect("static header"),
    );
    h.insert("x-content-type-options", "nosniff".parse().expect("static header"));
    h.insert("referrer-policy", "no-referrer".parse().expect("static header"));
    h.insert("cache-control", "no-store".parse().expect("static header"));
    resp
}

async fn index(State(st): State<Arc<UiState>>, Query(q): Query<HashMap<String, String>>, headers: HeaderMap) -> Response {
    if q.get("t").is_some_and(|t| t == &st.token) {
        let cookie = format!("{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict", st.token);
        return ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response();
    }
    if has_session(&headers, &st.token) {
        Html(content("index.html", INDEX_HTML)).into_response()
    } else {
        (StatusCode::UNAUTHORIZED, Html(content("locked.html", LOCKED_HTML))).into_response()
    }
}

/// The embedded file, or — for people working on the interface — the copy on disk when
/// `MA_UI_DEV_DIR` points at `apps/ui-server/static`, so edits show up on reload.
fn content(name: &'static str, embedded: &'static str) -> String {
    std::env::var("MA_UI_DEV_DIR")
        .ok()
        .and_then(|dir| std::fs::read_to_string(std::path::Path::new(&dir).join(name)).ok())
        .unwrap_or_else(|| embedded.to_string())
}

fn asset_response(content_type: &'static str, body: impl Into<String>) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body.into()).into_response()
}

async fn asset(Path(name): Path<String>) -> Response {
    match name.as_str() {
        "app.css" => asset_response("text/css; charset=utf-8", content("app.css", APP_CSS)),
        "app.js" => asset_response("text/javascript; charset=utf-8", content("app.js", APP_JS)),
        "rain.js" => asset_response("text/javascript; charset=utf-8", content("rain.js", RAIN_JS)),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------------------------------------------------------------------------- errors

pub(crate) struct ApiError(pub MaError);

impl From<MaError> for ApiError {
    fn from(e: MaError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.class {
            ErrorClass::NotFound => StatusCode::NOT_FOUND,
            ErrorClass::Policy => StatusCode::FORBIDDEN,
            ErrorClass::Validation | ErrorClass::Config | ErrorClass::Auth | ErrorClass::Unsupported => StatusCode::BAD_REQUEST,
            ErrorClass::Transient | ErrorClass::RateLimited => StatusCode::BAD_GATEWAY,
            ErrorClass::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = ma_security::redact::redact(&self.0.message, &[]);
        (status, Json(json!({ "error": { "class": format!("{:?}", self.0.class), "message": message } }))).into_response()
    }
}

pub(crate) type ApiResult = Result<Json<serde_json::Value>, ApiError>;

pub(crate) fn ok<T: serde::Serialize>(v: T) -> ApiResult {
    Ok(Json(serde_json::to_value(v).map_err(|e| MaError::internal(e.to_string()))?))
}
