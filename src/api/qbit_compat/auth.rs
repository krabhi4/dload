use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
};

use super::QbitState;

pub async fn login(
    State(state): State<QbitState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Some(ip) = crate::manager::extract_client_ip(&headers, Some(peer)) {
        if !state.manager.login_limiter.try_consume(ip).await {
            return (StatusCode::TOO_MANY_REQUESTS, "Fails.").into_response();
        }
    }
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

    // Always run bcrypt to prevent timing oracle on username existence.
    // Run DB lookup + bcrypt off the async runtime to avoid blocking the executor.
    const DUMMY_HASH: &str = "$2b$12$000000000000000000000uGKWMKFz95uGKWMKFz95uGKWMKFz9.";
    let repo = state.manager.repo.clone();
    let username_owned = username.to_string();
    let password_owned = password.to_string();
    let auth_result = tokio::task::spawn_blocking(move || {
        let (user, hash) = match repo.get_user_by_username(&username_owned) {
            Ok(Some(u)) => {
                let h = u.password_hash.clone();
                (Some(u), h)
            }
            _ => (None, DUMMY_HASH.to_string()),
        };
        let valid = bcrypt::verify(&password_owned, &hash).unwrap_or(false) && user.is_some();
        if valid {
            user
        } else {
            None
        }
    })
    .await
    .unwrap_or(None);

    let user = match auth_result {
        Some(u) => u,
        None => return (StatusCode::FORBIDDEN, "Fails.").into_response(),
    };

    let sid = state
        .sessions
        .create(user.username, user.role.as_str().to_string())
        .await;

    // Only stamp the cookie with `Secure` when we can see the client reached
    // the server over HTTPS (direct or via a reverse proxy that set
    // X-Forwarded-Proto). On plain-HTTP LAN deployments clients like
    // .NET's CookieContainer would otherwise refuse to send the cookie at
    // all, forcing Sonarr/Radarr back onto Basic Auth for every poll.
    let is_https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false);
    let cookie = if is_https {
        format!("SID={}; path=/; HttpOnly; SameSite=Lax; Secure", sid)
    } else {
        format!("SID={}; path=/; HttpOnly; SameSite=Lax", sid)
    };

    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
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
