use axum::body::Body;
use http::Request;

/// Helper to build a GET request for a given URI path.
pub fn request_get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}
