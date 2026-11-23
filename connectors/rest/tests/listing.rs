//! Regressions for reading lists from a REST API:
//!
//! * a server that caps the page size below what was asked for must still be read completely;
//! * a server that ignores the paging position must be reported, not read forever or cut short;
//! * "no such page" answers past the end finish the listing;
//! * an oversized response is refused while it arrives, not after it has been buffered.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ma_connector_rest::RestConnector;
use ma_types::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, Default)]
struct Api {
    /// Records in the collection.
    total: usize,
    /// The most records the server puts on one page, whatever the client asks for.
    cap: usize,
    /// Answer every request with the first page (the position parameter is ignored).
    ignores_position: bool,
    /// Answer 404 for a page past the last one instead of an empty list.
    not_found_past_end: bool,
    /// Answer 400 for an offset past the end instead of an empty list.
    bad_request_past_end: bool,
    /// Answer with an endless body. `Some(true)` announces its size, `Some(false)` does not.
    endless: Option<bool>,
}

struct Running {
    base: String,
    requests: Arc<AtomicUsize>,
    body_bytes_sent: Arc<AtomicUsize>,
}

fn query(target: &str, name: &str) -> Option<usize> {
    let q = target.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).and_then(|(_, v)| v.parse().ok()))
}

/// Tiny HTTP server for `GET /items?offset=&limit=` and `GET /items?page=&size=` (pages from 1).
async fn serve(api: Api) -> Running {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let body_bytes_sent = Arc::new(AtomicUsize::new(0));
    let (req_counter, sent_counter) = (requests.clone(), body_bytes_sent.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (req_counter, sent_counter) = (req_counter.clone(), sent_counter.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let mut n = 0;
                while !String::from_utf8_lossy(&buf[..n]).contains("\r\n\r\n") {
                    match sock.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => n += k,
                    }
                }
                req_counter.fetch_add(1, Ordering::SeqCst);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();

                if let Some(announced) = api.endless {
                    // far more than the connector's limit; stop when the client hangs up
                    const TOTAL: usize = 256 * 1024 * 1024;
                    let head = if announced {
                        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {TOTAL}\r\nConnection: close\r\n\r\n")
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n".to_string()
                    };
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let chunk = vec![b' '; 64 * 1024];
                    let mut sent = 0;
                    while sent < TOTAL {
                        if sock.write_all(&chunk).await.is_err() {
                            return;
                        }
                        sent += chunk.len();
                        sent_counter.fetch_add(chunk.len(), Ordering::SeqCst);
                    }
                    return;
                }

                let by_page = query(&target, "page").is_some();
                let asked = query(&target, "limit").or(query(&target, "size")).unwrap_or(api.cap).max(1);
                let size = asked.min(api.cap);
                let start = if api.ignores_position {
                    0
                } else if by_page {
                    (query(&target, "page").unwrap_or(1).max(1) - 1) * size
                } else {
                    query(&target, "offset").unwrap_or(0)
                };
                let past_end = start >= api.total && start > 0;
                let (status, body) = if past_end && by_page && api.not_found_past_end {
                    ("404 Not Found", r#"{"detail":"Invalid page."}"#.to_string())
                } else if past_end && !by_page && api.bad_request_past_end {
                    ("400 Bad Request", r#"{"error":"offset out of range"}"#.to_string())
                } else {
                    let items: Vec<String> =
                        (start..api.total.min(start + size)).map(|i| format!(r#"{{"id":{},"name":"item {}"}}"#, i + 1, i + 1)).collect();
                    ("200 OK", format!(r#"{{"items":[{}]}}"#, items.join(",")))
                };
                let out = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
            });
        }
    });
    Running { base: format!("http://{addr}"), requests, body_bytes_sent }
}

fn spec(pagination: Pagination) -> DeclarativeConnectorSpec {
    DeclarativeConnectorSpec {
        spec_version: 1,
        connector_id: "listing-test".into(),
        version: "1.0.0".into(),
        protocol: SpecProtocol::Rest,
        title: "listing".into(),
        api_version: None,
        base_path: String::new(),
        authentication: AuthSpec::None,
        rate_limit: None,
        resources: vec![ResourceSpec {
            name: "items".into(),
            identity_field: "id".into(),
            list: Some(ListOperation { path: "/items".into(), records_path: "items".into(), pagination }),
            create: None,
            update: None,
            get: None,
            lookup: None,
            fields: vec![],
            relationships: vec![],
        }],
        provenance: SpecProvenance { generated_by: "test".into(), generated_at: "now".into(), source_document_sha256: None, notes: vec![] },
    }
}

fn by_offset() -> Pagination {
    Pagination::Offset { offset_param: "offset".into(), limit_param: "limit".into(), page_size: 200 }
}

fn by_page() -> Pagination {
    Pagination::Page { page_param: "page".into(), size_param: Some("size".into()), page_size: 200, first_page: 1 }
}

async fn connect(base: &str, pagination: Pagination) -> RestConnector {
    RestConnector::connect(
        spec(pagination),
        &serde_json::json!({"base_url": base, "allow_private_network": true}),
        &["127.0.0.1".to_string()],
        None,
    )
    .await
    .unwrap()
}

/// Read every page the way a migration does. Returns the record ids in order.
async fn read_all(conn: RestConnector) -> MaResult<Vec<String>> {
    let handle = conn.into_handle();
    let source = handle.source.clone().unwrap();
    let mut ids = vec![];
    let mut cursor: Option<Cursor> = None;
    for _ in 0..1000 {
        let b = source.read_batch("items", cursor.as_ref(), 200).await?;
        ids.extend(b.records.into_iter().map(|r| r.id));
        match b.next_cursor {
            Some(c) => cursor = Some(c),
            None => return Ok(ids),
        }
    }
    panic!("the listing never ended");
}

fn expected(n: usize) -> Vec<String> {
    (1..=n).map(|i| i.to_string()).collect()
}

/// The server puts at most 3 records on a page although 200 were asked for. Every page is
/// therefore "short"; all 10 records must still be read.
#[tokio::test(flavor = "multi_thread")]
async fn a_capped_page_size_does_not_cut_the_listing_short() {
    for pagination in [by_offset(), by_page()] {
        let api = serve(Api { total: 10, cap: 3, ..Default::default() }).await;
        let ids = read_all(connect(&api.base, pagination).await).await.unwrap();
        assert_eq!(ids, expected(10), "all records are read, in order, none twice");
        assert_eq!(api.requests.load(Ordering::SeqCst), 5, "four pages with records and one empty page that ends the listing");
    }
}

/// When the collection size is an exact multiple of the page size, the listing ends on the empty
/// page after the last full one.
#[tokio::test(flavor = "multi_thread")]
async fn an_exact_multiple_of_the_page_size_ends_cleanly() {
    for pagination in [by_offset(), by_page()] {
        let api = serve(Api { total: 9, cap: 3, ..Default::default() }).await;
        assert_eq!(read_all(connect(&api.base, pagination).await).await.unwrap(), expected(9));
    }
    let empty = serve(Api { total: 0, cap: 3, ..Default::default() }).await;
    assert!(read_all(connect(&empty.base, by_offset()).await).await.unwrap().is_empty());
}

/// A server that ignores the position would hand out its first page forever. That is reported
/// as an error on the second request; it is neither read endlessly nor treated as complete.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_ignores_the_position_is_reported() {
    for pagination in [by_offset(), by_page()] {
        let api = serve(Api { total: 10, cap: 3, ignores_position: true, ..Default::default() }).await;
        let e = read_all(connect(&api.base, pagination).await).await.unwrap_err();
        assert!(e.message.contains("returned the same page twice"), "{}", e.message);
        assert_eq!(api.requests.load(Ordering::SeqCst), 2);
    }
}

/// Frameworks answer 404 for a page number past the last one, and some APIs answer 400 for an
/// offset past the end after a page that was not full. Both end the listing.
#[tokio::test(flavor = "multi_thread")]
async fn no_such_page_past_the_end_finishes_the_listing() {
    for total in [9, 10] {
        let api = serve(Api { total, cap: 3, not_found_past_end: true, ..Default::default() }).await;
        assert_eq!(read_all(connect(&api.base, by_page()).await).await.unwrap(), expected(total));
    }
    let api = serve(Api { total: 10, cap: 3, bad_request_past_end: true, ..Default::default() }).await;
    assert_eq!(read_all(connect(&api.base, by_offset()).await).await.unwrap(), expected(10));
    // ... but an error for the very first page is an error
    let api = serve(Api { total: 0, cap: 3, not_found_past_end: true, ..Default::default() }).await;
    assert!(read_all(connect(&api.base, by_page()).await).await.unwrap().is_empty(), "an empty first page is simply empty");
}

/// The size limit is enforced while the body arrives. The server offers 256 MiB; the connector
/// must hang up shortly after its 32 MiB limit instead of buffering everything first.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_response_is_refused_while_it_arrives() {
    const LIMIT: usize = 32 * 1024 * 1024;
    // size not announced: stopped while streaming
    let api = serve(Api { endless: Some(false), ..Default::default() }).await;
    let e = read_all(connect(&api.base, by_offset()).await).await.unwrap_err();
    assert!(e.message.contains("larger than 32 MiB"), "{}", e.message);
    assert_eq!(e.class, ErrorClass::Validation);
    let sent = api.body_bytes_sent.load(Ordering::SeqCst);
    assert!(sent < 2 * LIMIT, "download was cut off near the limit, but the server got to send {} MiB", sent / (1024 * 1024));
    // size announced: refused before the body is read
    let api = serve(Api { endless: Some(true), ..Default::default() }).await;
    let e = read_all(connect(&api.base, by_offset()).await).await.unwrap_err();
    assert!(e.message.contains("larger than 32 MiB"), "{}", e.message);
    let sent = api.body_bytes_sent.load(Ordering::SeqCst);
    assert!(sent < LIMIT, "nothing near the limit was downloaded, but the server got to send {} MiB", sent / (1024 * 1024));
}
