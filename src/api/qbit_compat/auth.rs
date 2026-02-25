use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
};

use super::QbitState;

pub async fn login(
    State(state): State<QbitState>,
    body: String,
) -> impl IntoResponse {
    // Parse form-encoded body: username=X&password=Y
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();

    let username = params
        .iter()
        .find(|(k, _)| k == "username")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let password = params
        .iter()
        .find(|(k, _)| k == "password")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    if username.is_empty() || password.is_empty() {
        return (StatusCode::FORBIDDEN, "Fails.").into_response();
    }

    // Validate against the same user database as native auth
    let user = match state.manager.repo.get_user_by_username(username) {
        Ok(Some(u)) => u,
        _ => return (StatusCode::FORBIDDEN, "Fails.").into_response(),
    };

    if !bcrypt::verify(password, &user.password_hash).unwrap_or(false) {
        return (StatusCode::FORBIDDEN, "Fails.").into_response();
    }

    let sid = state
        .sessions
        .create(user.username, user.role.as_str().to_string())
        .await;

    (
        StatusCode::OK,
        [(header::SET_COOKIE, format!("SID={}; path=/; HttpOnly; SameSite=Strict", sid))],
        "Ok.",
    )
        .into_response()
}

pub async fn logout(
    State(state): State<QbitState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Some(sid) = extract_sid(&headers) {
        state.sessions.remove(&sid).await;
    }
    (StatusCode::OK, "Ok.").into_response()
}

pub fn extract_sid(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some(sid) = part.strip_prefix("SID=") {
            return Some(sid.to_string());
        }
    }
    None
}
