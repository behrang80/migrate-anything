//! Studio (the web interface) end to end, driven over HTTP the way the browser drives it:
//! sign-in link → demo data → mapping → plan → test run → approval with the terminal code →
//! run → verification → report. It also checks what the interface must refuse.
//!
//! Approvals here use the test-only session (`MA_INSECURE_TEST_APPROVAL=1` plus the
//! `test-approval` cargo feature), which release binaries do not contain.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ma_core::{App, InteractiveSession};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Studio {
    ui: ma_ui::RunningUi,
    http: reqwest::Client,
    base: String,
    _dir: tempfile::TempDir,
}

impl Studio {
    /// Start Studio on a free port. `terminal` says whether it believes a person started it
    /// from an interactive terminal (only then can anything be approved).
    async fn start(terminal: bool) -> Studio {
        common::init_env();
        ma_security::network::install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(&dir.path().join("home")).await.unwrap();
        let session = terminal.then(|| InteractiveSession::for_tests().unwrap());
        let ui = ma_ui::serve(Arc::new(app), ma_ui::UiOptions { port: 0, session }).await.unwrap();
        let http = reqwest::Client::builder().cookie_store(true).build().unwrap();
        let base = format!("http://127.0.0.1:{}", ui.port);
        Studio { ui, http, base, _dir: dir }
    }

    /// Open the link printed in the terminal: this is what signs the browser in.
    async fn sign_in(&self) {
        let page = self.http.get(&self.ui.url).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert!(page.text().await.unwrap().contains("/assets/app.js"), "the app page is served after sign-in");
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let r = self.http.get(format!("{}/api{path}", self.base)).send().await.unwrap();
        (r.status(), r.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let r = self.http.post(format!("{}/api{path}", self.base)).header("origin", &self.base).json(&body).send().await.unwrap();
        (r.status(), r.json().await.unwrap_or(Value::Null))
    }

    async fn post_ok(&self, path: &str, body: Value) -> Value {
        let (status, v) = self.post(path, body).await;
        assert_eq!(status, StatusCode::OK, "POST {path}: {v}");
        v
    }

    /// Demo connections, suggested mapping and a plan that passed its test run.
    async fn demo_plan(&self, batch_size: usize) -> (String, u64) {
        let demo = self.post_ok("/demo", json!({})).await;
        let total = demo["records"].as_u64().unwrap();
        let suggested = self.post_ok("/mapping/suggest", json!({"source": demo["source"], "destination": demo["destination"]})).await;
        assert_eq!(suggested["destination_creates_objects"], true);
        assert_eq!(suggested["suggestion"]["mapping"]["objects"].as_array().unwrap().len(), 4);
        let plan = self
            .post_ok(
                "/plans",
                json!({"source": demo["source"], "destination": demo["destination"], "mapping": suggested["suggestion"]["mapping"], "batch_size": batch_size}),
            )
            .await;
        assert_eq!(plan["approvable"], true, "{plan}");
        let plan_id = plan["plan_id"].as_str().unwrap().to_string();
        let dry = self.post_ok(&format!("/plans/{plan_id}/dry-run"), json!({})).await;
        assert_eq!(dry["passed"], true, "{dry}");
        assert_eq!(dry["wrote_to_destination"], false);
        (plan_id, total)
    }

    /// Ask for a code "on the terminal" and type it in, as the operator would.
    async fn approve(&self, plan_id: &str) {
        let ch = self.post_ok("/challenges", json!({"kind": "plan", "subject": plan_id})).await;
        let id = ch["challenge_id"].as_str().unwrap();
        let code = self.ui.challenge_code(id).expect("a code is pending");
        let status = self.post_ok(&format!("/plans/{plan_id}/approve"), json!({"challenge_id": id, "code": code})).await;
        assert_eq!(status["state"], "approved", "{status}");
    }

    async fn wait_for_status(&self, job_id: &str, wanted: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let (_, job) = self.get(&format!("/jobs/{job_id}")).await;
            let status = job["status"].as_str().unwrap_or_default().to_string();
            if status == wanted {
                return job;
            }
            assert!(!(status == "failed" || status == "cancelled"), "job ended as {status}: {job}");
            assert!(Instant::now() < deadline, "job did not become {wanted} in time: {job}");
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    async fn verified_report(&self, job_id: &str, total: u64) {
        self.post_ok(&format!("/jobs/{job_id}/verify"), json!({})).await;
        let (_, rep) = self.get(&format!("/jobs/{job_id}/report")).await;
        let r = &rep["report"];
        assert_eq!(r["final_status"], "completed_and_verified", "{}", rep["text"]);
        assert_eq!(r["records_written"], total);
        assert_eq!(r["records_verified"], total);
        assert_eq!(r["records_failed"], 0);
        assert!(rep["text"].as_str().unwrap().contains("MIGRATION REPORT"));
        let (_, verification) = self.get(&format!("/jobs/{job_id}/verification")).await;
        let checks = verification["checks"].as_array().unwrap();
        assert!(!checks.is_empty() && checks.iter().all(|c| c["passed"] == true), "{verification}");
        let (_, exceptions) = self.get(&format!("/jobs/{job_id}/exceptions?limit=10")).await;
        assert_eq!(exceptions.as_array().map(Vec::len), Some(0));
    }
}

fn message(v: &Value) -> &str {
    v["error"]["message"].as_str().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn studio_runs_the_demo_from_sign_in_to_verified_report() {
    let s = Studio::start(true).await;

    // --- before sign-in nothing is reachable
    let locked = s.http.get(&s.base).send().await.unwrap();
    assert_eq!(locked.status(), StatusCode::UNAUTHORIZED);
    assert!(!locked.text().await.unwrap().contains("/assets/app.js"), "the app itself is not served before sign-in");
    let (status, v) = s.get("/state").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(message(&v).contains("not signed in"), "{v}");
    let wrong = s.http.get(format!("{}/?t=not-the-token", s.base)).send().await.unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    s.sign_in().await;

    // --- the page ships with a strict content policy and its own scripts only
    let js = s.http.get(format!("{}/assets/app.js", s.base)).send().await.unwrap();
    assert_eq!(js.status(), StatusCode::OK);
    assert!(js.headers()["content-type"].to_str().unwrap().starts_with("text/javascript"));
    assert!(js.headers()["content-security-policy"].to_str().unwrap().contains("default-src 'self'"));
    assert!(js.text().await.unwrap().contains("CodeRain"));
    assert_eq!(s.http.get(format!("{}/assets/nope.js", s.base)).send().await.unwrap().status(), StatusCode::NOT_FOUND);

    // --- another website cannot drive the interface: wrong Origin and wrong Host are refused
    let cross = s.http.post(format!("{}/api/demo", s.base)).header("origin", "http://evil.example").json(&json!({})).send().await.unwrap();
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    let no_origin = s.http.post(format!("{}/api/demo", s.base)).json(&json!({})).send().await.unwrap();
    assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);
    let rebinding = s.http.get(format!("{}/api/state", s.base)).header("host", "evil.example").send().await.unwrap();
    assert_eq!(rebinding.status(), StatusCode::FORBIDDEN);

    let (status, state) = s.get("/state").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["approvals"]["enabled"], true);
    assert_eq!(state["connections"].as_array().map(Vec::len), Some(0));

    // --- demo data, mapping, plan, test run
    let (plan_id, total) = s.demo_plan(5_000).await;
    assert_eq!(total, 90_000);
    let (_, state) = s.get("/state").await;
    assert_eq!(state["connections"].as_array().map(Vec::len), Some(2));

    // --- nothing runs without approval
    let (status, v) = s.post("/jobs", json!({"plan_id": plan_id})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{v}");

    // --- the approval code: wrong codes, codes for something else and used codes are refused
    let ch = s.post_ok("/challenges", json!({"kind": "plan", "subject": plan_id})).await;
    let id = ch["challenge_id"].as_str().unwrap().to_string();
    let (status, v) = s.post(&format!("/plans/{plan_id}/approve"), json!({"challenge_id": id, "code": "000000"})).await;
    assert_ne!(status, StatusCode::OK, "a wrong code must not approve: {v}");
    let other = s.post_ok("/challenges", json!({"kind": "connector", "subject": "some-connector@1"})).await;
    let other_id = other["challenge_id"].as_str().unwrap().to_string();
    let other_code = s.ui.challenge_code(&other_id).unwrap();
    let (status, v) = s.post(&format!("/plans/{plan_id}/approve"), json!({"challenge_id": other_id, "code": other_code})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(message(&v).contains("issued for something else"), "{v}");
    let (_, plan) = s.get(&format!("/plans/{plan_id}")).await;
    assert_ne!(plan["approval"]["state"], "approved");

    let code = s.ui.challenge_code(&id).unwrap();
    let approved = s.post_ok(&format!("/plans/{plan_id}/approve"), json!({"challenge_id": id, "code": code})).await;
    assert_eq!(approved["state"], "approved");
    assert!(approved["detail"].as_str().unwrap().contains("terminal-code-via-local-ui"), "{approved}");
    let (status, v) = s.post(&format!("/plans/{plan_id}/approve"), json!({"challenge_id": id, "code": code})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a code works once: {v}");

    // --- run, verify, report
    let job = s.post_ok("/jobs", json!({"plan_id": plan_id})).await;
    let job_id = job["job_id"].as_str().unwrap().to_string();
    let done = s.wait_for_status(&job_id, "completed").await;
    assert_eq!(done["stages"].as_array().unwrap().len(), 4);
    s.verified_report(&job_id, total).await;
    let (status, _) = s.post(&format!("/jobs/{job_id}/explode"), json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    s.ui.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn studio_pauses_and_resumes_a_running_migration() {
    let s = Studio::start(true).await;
    s.sign_in().await;
    // small batches, so the run is long enough to be interrupted
    let (plan_id, total) = s.demo_plan(100).await;
    s.approve(&plan_id).await;

    let job = s.post_ok("/jobs", json!({"plan_id": plan_id})).await;
    let job_id = job["job_id"].as_str().unwrap().to_string();
    s.post_ok(&format!("/jobs/{job_id}/pause"), json!({})).await;
    let paused = s.wait_for_status(&job_id, "paused").await;
    let written =
        |job: &Value| -> u64 { job["stages"].as_array().unwrap().iter().map(|st| st["counters"]["read"].as_u64().unwrap_or(0)).sum() };
    let at_pause = written(&paused);
    assert!(at_pause < total, "the job was paused part-way ({at_pause} of {total})");

    // while paused nothing moves
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (_, still) = s.get(&format!("/jobs/{job_id}")).await;
    assert_eq!(still["status"], "paused");
    assert_eq!(written(&still), at_pause);

    s.post_ok(&format!("/jobs/{job_id}/resume"), json!({})).await;
    s.wait_for_status(&job_id, "completed").await;
    s.verified_report(&job_id, total).await;
    s.ui.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn studio_without_a_terminal_cannot_approve() {
    let s = Studio::start(false).await;
    s.sign_in().await;
    let (_, state) = s.get("/state").await;
    assert_eq!(state["approvals"]["enabled"], false);

    let (plan_id, _) = s.demo_plan(5_000).await;
    let (status, v) = s.post("/challenges", json!({"kind": "plan", "subject": plan_id})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(message(&v).contains("approvals are disabled"), "{v}");
    let (status, _) = s.post(&format!("/plans/{plan_id}/approve"), json!({"challenge_id": "nope", "code": "123456"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s.post("/jobs", json!({"plan_id": plan_id})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an unapproved plan never runs");
    s.ui.stop();
}
