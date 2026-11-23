//! Regression: a non-idempotent create must never be re-sent when its outcome is unknown.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ma_connector_rest::RestConnector;
use ma_connector_sdk::{SinkObjectSpec, WriteRequest};
use ma_types::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Minimal HTTP server that answers every request with 502 and counts POSTs.
async fn failing_server() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let counter = posts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let mut n = 0;
                // read headers (+ small body); enough for these tiny requests
                loop {
                    match sock.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => break,
                        Ok(k) => n += k,
                    }
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if n >= head_end + 4 + len {
                            break;
                        }
                    }
                }
                if buf.starts_with(b"POST") {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let _ = sock.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            });
        }
    });
    (format!("http://{addr}"), posts)
}

fn spec() -> DeclarativeConnectorSpec {
    DeclarativeConnectorSpec {
        spec_version: 1,
        connector_id: "retry-test".into(),
        version: "1.0.0".into(),
        protocol: SpecProtocol::Rest,
        title: "retry".into(),
        api_version: None,
        base_path: String::new(),
        authentication: AuthSpec::None,
        rate_limit: None,
        resources: vec![ResourceSpec {
            name: "items".into(),
            identity_field: "id".into(),
            list: None,
            create: Some(WriteOperation {
                method: HttpMethod::Post,
                path: "/items".into(),
                response_id_path: String::new(),
                idempotent: false,
            }),
            update: None,
            get: None,
            lookup: None,
            fields: vec![],
            relationships: vec![],
        }],
        provenance: SpecProvenance { generated_by: "test".into(), generated_at: "now".into(), source_document_sha256: None, notes: vec![] },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn non_idempotent_create_is_not_resent_on_5xx() {
    let (base, posts) = failing_server().await;
    let conn = RestConnector::connect(
        spec(),
        &serde_json::json!({"base_url": base, "allow_private_network": true}),
        &["127.0.0.1".to_string()],
        None,
    )
    .await
    .unwrap();
    let handle = conn.into_handle();
    let sink = handle.sink.clone().unwrap();
    sink.prepare(&SinkObjectSpec { object: "items".into(), fields: vec![], key_field: None, mode: WriteMode::Insert }, None).await.unwrap();
    let out = sink
        .write_batch(
            "items",
            vec![WriteRequest { source_id: "1".into(), fields: [("id".to_string(), Value::Int(1))].into_iter().collect() }],
        )
        .await
        .unwrap();
    let err = out[0].as_ref().unwrap_err();
    assert!(err.message.contains("not re-sent"), "{}", err.message);
    assert_eq!(posts.load(Ordering::SeqCst), 1, "the create was sent exactly once");
}
