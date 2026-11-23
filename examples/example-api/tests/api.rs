//! End-to-end tests over real HTTP using a minimal HTTP/1.1 client.

use std::collections::HashSet;

use ma_example_api::{ApiConfig, PROMPT_INJECTION_FIXTURE, RunningApi, start};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const KEY: &str = "test-key-123";

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("body is not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
}

fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = raw.windows(2).position(|w| w == b"\r\n").expect("chunk size");
        let size_str = std::str::from_utf8(&raw[..line_end]).expect("utf8");
        let size = usize::from_str_radix(size_str.split(';').next().unwrap().trim(), 16).expect("chunk size hex");
        raw = &raw[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

async fn request(api: &RunningApi, method: &str, path: &str, key: Option<&str>, body: Option<&Value>) -> HttpResponse {
    let addr = api.base_url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let body_bytes = body.map(|b| serde_json::to_vec(b).unwrap()).unwrap_or_default();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(k) = key {
        req.push_str(&format!("X-Api-Key: {k}\r\n"));
    }
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body_bytes.len()));
    stream.write_all(req.as_bytes()).await.expect("write head");
    stream.write_all(&body_bytes).await.expect("write body");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");

    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("header terminator");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status: u16 = status_line.split_whitespace().nth(1).expect("status code").parse().expect("numeric status");
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).collect();
    let mut body = raw[split + 4..].to_vec();
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked"));
    if chunked {
        body = dechunk(&body);
    }
    HttpResponse { status, headers, body }
}

async fn get(api: &RunningApi, path: &str) -> HttpResponse {
    request(api, "GET", path, Some(KEY), None).await
}

async fn post(api: &RunningApi, path: &str, body: &Value) -> HttpResponse {
    request(api, "POST", path, Some(KEY), Some(body)).await
}

/// Walks a collection with cursor pagination and returns every item.
async fn walk(api: &RunningApi, collection: &str, limit: usize) -> (Vec<Value>, usize) {
    let mut items = Vec::new();
    let mut pages = 0;
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            None => format!("/api/v2/{collection}?limit={limit}"),
            Some(c) => format!("/api/v2/{collection}?limit={limit}&after={c}"),
        };
        let resp = get(api, &path).await;
        assert_eq!(resp.status, 200, "{path}");
        let page = resp.json();
        let page_items = page["items"].as_array().expect("items array");
        assert!(page_items.len() <= limit);
        items.extend(page_items.iter().cloned());
        pages += 1;
        match &page["next_cursor"] {
            Value::Null => break,
            Value::String(c) => cursor = Some(c.clone()),
            other => panic!("bad next_cursor: {other}"),
        }
        assert!(pages < 1000, "pagination does not terminate");
    }
    (items, pages)
}

#[tokio::test]
async fn openapi_document_is_complete_and_public() {
    let api = start(ApiConfig::default()).await.unwrap();
    let resp = request(&api, "GET", "/openapi.json", None, None).await;
    assert_eq!(resp.status, 200);
    let doc = resp.json();
    assert_eq!(doc["openapi"], "3.0.3");
    assert_eq!(doc["info"]["title"], "FieldOps Legacy Service");
    assert_eq!(doc["info"]["version"], "2.4.1");
    assert_eq!(doc["servers"][0]["url"], "/api/v2");
    for path in ["/technicians", "/technicians/{tech_id}", "/work_orders", "/work_orders/{wo_number}"] {
        assert!(doc["paths"][path].is_object(), "missing path {path}");
    }
    assert!(doc["paths"]["/technicians"]["post"].is_object());
    assert!(doc["paths"]["/work_orders"]["post"].is_object());
    for schema in ["Technician", "TechnicianInput", "WorkOrder", "WorkOrderInput", "Site", "TechnicianPage", "WorkOrderPage"] {
        assert!(doc["components"]["schemas"][schema].is_object(), "missing schema {schema}");
    }
    assert_eq!(doc["components"]["securitySchemes"]["ApiKeyAuth"]["name"], "X-Api-Key");
    let tech_params = doc["paths"]["/technicians"]["get"]["parameters"].as_array().unwrap();
    let names: Vec<&str> = tech_params.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["limit", "after", "tech_id"]);
    assert_eq!(
        doc["components"]["schemas"]["WorkOrder"]["properties"]["assigned_tech"]["description"],
        "tech_id of the assigned technician"
    );
    // Injection fixture present in both required places.
    assert!(doc["info"]["description"].as_str().unwrap().contains(PROMPT_INJECTION_FIXTURE));
    assert!(
        doc["components"]["schemas"]["WorkOrder"]["properties"]["title"]["description"]
            .as_str()
            .unwrap()
            .contains("ignore all previous instructions")
    );
    // Not counted as an /api/v2 request.
    assert_eq!(api.request_count(), 0);
    api.shutdown();
}

#[tokio::test]
async fn requires_api_key() {
    let api = start(ApiConfig::default()).await.unwrap();
    for key in [None, Some("wrong-key")] {
        let resp = request(&api, "GET", "/api/v2/technicians", key, None).await;
        assert_eq!(resp.status, 401);
        assert_eq!(resp.json(), json!({ "error": "unauthorized" }));
    }
    let resp = request(&api, "GET", "/api/v2/technicians", Some(KEY), None).await;
    assert_eq!(resp.status, 200);
    assert_eq!(api.request_count(), 3);
    api.shutdown();
}

#[tokio::test]
async fn pagination_walks_every_record_exactly_once() {
    let api = start(ApiConfig::default()).await.unwrap();

    let (techs, pages) = walk(&api, "technicians", 7).await;
    assert_eq!(techs.len(), 40);
    assert_eq!(pages, 6); // 7*5 = 35, then 5 more
    let ids: HashSet<&str> = techs.iter().map(|t| t["tech_id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 40, "duplicate technicians");
    assert_eq!(techs, api.technicians());

    // Default limit 25.
    let first = get(&api, "/api/v2/work_orders").await.json();
    assert_eq!(first["items"].as_array().unwrap().len(), 25);
    // Limit capped silently at 100.
    let capped = get(&api, "/api/v2/work_orders?limit=500").await.json();
    assert_eq!(capped["items"].as_array().unwrap().len(), 100);

    let (wos, _) = walk(&api, "work_orders", 100).await;
    assert_eq!(wos.len(), 120);
    let nums: HashSet<i64> = wos.iter().map(|w| w["wo_number"].as_i64().unwrap()).collect();
    assert_eq!(nums.len(), 120, "duplicate work orders");
    assert_eq!(wos, api.work_orders());

    let bad = get(&api, "/api/v2/work_orders?after=zz").await;
    assert_eq!(bad.status, 400);
    api.shutdown();
}

#[tokio::test]
async fn seed_data_is_realistic_and_referentially_intact() {
    let api = start(ApiConfig::default()).await.unwrap();
    let techs = api.technicians();
    let ids: HashSet<&str> = techs.iter().map(|t| t["tech_id"].as_str().unwrap()).collect();
    assert!(techs.iter().any(|t| t["email"].is_null()));
    assert!(techs.iter().any(|t| t["x_badge_color"].is_string()));
    assert!(techs.iter().all(|t| t["hourly_rate"].is_f64()));
    let wos = api.work_orders();
    assert!(wos.iter().any(|w| w["assigned_tech"].is_null()));
    for w in &wos {
        if let Some(t) = w["assigned_tech"].as_str() {
            assert!(ids.contains(t), "dangling assigned_tech {t}");
        }
        assert!(w["site"]["city"].is_string());
        assert!(w["tags"].is_array());
    }
    api.shutdown();
}

#[tokio::test]
async fn identity_filter_and_get_by_id() {
    let api = start(ApiConfig::default()).await.unwrap();
    let page = get(&api, "/api/v2/technicians?tech_id=T-0007").await.json();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["tech_id"], "T-0007");
    assert!(page["next_cursor"].is_null());

    let none = get(&api, "/api/v2/technicians?tech_id=T-9999").await.json();
    assert_eq!(none["items"].as_array().unwrap().len(), 0);

    let wo = api.work_orders()[3].clone();
    let num = wo["wo_number"].as_i64().unwrap();
    let page = get(&api, &format!("/api/v2/work_orders?wo_number={num}")).await.json();
    assert_eq!(page["items"], json!([wo]));

    let one = get(&api, &format!("/api/v2/work_orders/{num}")).await;
    assert_eq!(one.status, 200);
    assert_eq!(one.json(), wo);
    let t = get(&api, "/api/v2/technicians/T-0001").await;
    assert_eq!(t.status, 200);
    assert_eq!(t.json()["tech_id"], "T-0001");
    assert_eq!(get(&api, "/api/v2/technicians/T-9999").await.status, 404);
    assert_eq!(get(&api, "/api/v2/work_orders/1").await.status, 404);
    api.shutdown();
}

#[tokio::test]
async fn create_conflict_and_validation() {
    let api = start(ApiConfig { empty: true, ..ApiConfig::default() }).await.unwrap();
    assert!(api.technicians().is_empty());
    assert!(api.work_orders().is_empty());

    let tech = json!({
        "tech_id": "T-0500",
        "full_name": "Zoë Søren",
        "email": null,
        "skill_level": "lead",
        "hourly_rate": 62.75,
        "active": true,
        "hired_on": "2019-04-12",
        "region_code": "NW-01",
        "x_badge_color": "teal"
    });
    let created = post(&api, "/api/v2/technicians", &tech).await;
    assert_eq!(created.status, 201);
    assert_eq!(created.json(), tech);
    assert_eq!(api.technicians(), vec![tech.clone()]);

    let dup = post(&api, "/api/v2/technicians", &tech).await;
    assert_eq!(dup.status, 409);
    assert!(dup.json()["error"].is_string());

    let mut bad_enum = tech.clone();
    bad_enum["tech_id"] = json!("T-0501");
    bad_enum["skill_level"] = json!("wizard");
    let resp = post(&api, "/api/v2/technicians", &bad_enum).await;
    assert_eq!(resp.status, 422);
    assert!(resp.json()["error"].as_str().unwrap().contains("skill_level"));

    let mut missing = tech.clone();
    missing.as_object_mut().unwrap().remove("tech_id");
    assert_eq!(post(&api, "/api/v2/technicians", &missing).await.status, 422);

    let wo = json!({
        "title": "Repair boiler",
        "status": "open",
        "assigned_tech": "T-0500",
        "priority": 2,
        "estimated_hours": 3.5,
        "site": { "street": "1 Main St", "city": "Malmö", "postal_code": "21000" },
        "opened_at": "2024-05-01T08:30:00Z",
        "tags": ["vip"]
    });
    let created = post(&api, "/api/v2/work_orders", &wo).await;
    assert_eq!(created.status, 201);
    let body = created.json();
    assert_eq!(body["wo_number"], 100_001);
    assert_eq!(body["title"], "Repair boiler");

    let mut explicit = wo.clone();
    explicit["wo_number"] = json!(4242);
    explicit["opened_at"] = json!("2024-05-01T08:30:00.123+02:00");
    assert_eq!(post(&api, "/api/v2/work_orders", &explicit).await.status, 201);
    assert_eq!(post(&api, "/api/v2/work_orders", &explicit).await.status, 409);

    let mut bad_status = wo.clone();
    bad_status["status"] = json!("pending");
    assert_eq!(post(&api, "/api/v2/work_orders", &bad_status).await.status, 422);

    let mut dangling = wo.clone();
    dangling["assigned_tech"] = json!("T-0999");
    assert_eq!(post(&api, "/api/v2/work_orders", &dangling).await.status, 422);

    let not_json = request(&api, "POST", "/api/v2/work_orders", Some(KEY), None).await;
    assert_eq!(not_json.status, 422);

    assert_eq!(api.work_orders().len(), 2);
    api.shutdown();
}

#[tokio::test]
async fn rate_limiting_every_nth_request() {
    let api = start(ApiConfig { rate_limit_every: Some(3), empty: true, ..ApiConfig::default() }).await.unwrap();
    let mut statuses = Vec::new();
    for i in 0..9 {
        let tech = json!({
            "tech_id": format!("T-{:04}", i + 1),
            "full_name": "Rate Limited",
            "skill_level": "junior",
            "hourly_rate": 45.5,
            "hired_on": "2020-01-01",
            "region_code": "SE-03"
        });
        let resp = post(&api, "/api/v2/technicians", &tech).await;
        if resp.status == 429 {
            assert_eq!(resp.header("retry-after"), Some("1"));
        }
        statuses.push(resp.status);
    }
    assert_eq!(statuses, [201, 201, 429, 201, 201, 429, 201, 201, 429]);
    assert_eq!(api.request_count(), 9);
    assert_eq!(api.rate_limited_count(), 3);
    // 429s have no side effects.
    assert_eq!(api.technicians().len(), 6);
    api.shutdown();
}

#[tokio::test]
async fn data_is_deterministic_per_seed() {
    let a = start(ApiConfig::default()).await.unwrap();
    let b = start(ApiConfig::default()).await.unwrap();
    let c = start(ApiConfig { seed: 7, ..ApiConfig::default() }).await.unwrap();
    assert_eq!(a.technicians(), b.technicians());
    assert_eq!(a.work_orders(), b.work_orders());
    assert_ne!(a.technicians(), c.technicians());
    // Also identical over HTTP.
    let (wa, _) = walk(&a, "work_orders", 50).await;
    let (wb, _) = walk(&b, "work_orders", 50).await;
    assert_eq!(wa, wb);
    a.shutdown();
    b.shutdown();
    c.shutdown();
}
