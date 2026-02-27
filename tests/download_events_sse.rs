use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn downloads_events_endpoint_exists() {
    // The SSE endpoint does not require auth — it streams public status updates.
    // We only verify the route is registered and returns 200 with the correct content-type.
    let app = dload::server::build_app_for_test().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/downloads/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/event-stream"),
        "expected text/event-stream, got: {ct}"
    );
}
