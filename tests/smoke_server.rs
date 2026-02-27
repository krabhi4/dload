mod common;

use tower::ServiceExt;

#[tokio::test]
async fn healthz_route_is_registered() {
    let app = dload::server::build_app_for_test().await;
    let response = app
        .oneshot(common::request_get("/healthz"))
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);
}
