mod common;

use axum::body::to_bytes;
use tower::ServiceExt;

#[tokio::test]
async fn healthz_returns_ok() {
    let app = dload::server::build_app_for_test().await;
    let res = app.oneshot(common::request_get("/healthz")).await.unwrap();
    assert_eq!(res.status(), http::StatusCode::OK);
}

#[tokio::test]
async fn readyz_returns_json_payload() {
    let app = dload::server::build_app_for_test().await;
    let res = app.oneshot(common::request_get("/readyz")).await.unwrap();
    assert_eq!(res.status(), http::StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(res.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ready");
}
