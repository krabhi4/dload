use crate::domain::{jwt_secret, Claims, Role, User};
use crate::manager::SharedState;
use axum::{
    extract::State,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use bcrypt::{hash, verify};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/login", post(login))
        .route("/api/auth/register", post(register))
        .route("/api/auth/verify", post(verify_token))
        .route("/api/auth/profile", post(change_password))
        .route("/api/auth/me", post(get_profile))
        .route("/api/auth/users", get(list_users).post(create_user))
        .route("/api/auth/users/{id}", delete(delete_user))
        .route(
            "/api/auth/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route("/api/auth/api-keys/{id}", delete(delete_api_key))
        .with_state(state)
}

const MAX_API_KEYS_PER_USER: i64 = 50;

#[derive(Debug, serde::Deserialize)]
struct CreateApiKeyRequest {
    name: String,
}

fn bearer_token(headers: &axum::http::HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

#[derive(Debug, serde::Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, serde::Deserialize)]
struct RegisterRequest {
    username: String,
    password: String,
}

#[derive(Debug, serde::Deserialize)]
struct CreateUserRequest {
    token: String,
    username: String,
    password: String,
    role: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ChangePasswordRequest {
    token: String,
    current_password: String,
    new_password: String,
}

async fn auth_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let has_users = state
        .repo_blocking(|repo| repo.get_all_users().map(|u| !u.is_empty()).unwrap_or(false))
        .await;
    Json(serde_json::json!({
        "needs_setup": !has_users
    }))
}

async fn register(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<RegisterRequest>,
) -> Json<serde_json::Value> {
    if let Some(ip) = crate::manager::extract_client_ip(&headers, Some(peer)) {
        if !state.login_limiter.try_consume(ip).await {
            return Json(serde_json::json!({
                "success": false,
                "error": "Too many attempts. Try again later."
            }));
        }
    }

    if payload.password.len() < 4 {
        return Json(serde_json::json!({
            "success": false,
            "error": "Password must be at least 4 characters"
        }));
    }
    let username = payload.username.trim().to_string();
    if username.is_empty() || username.len() > 64 {
        return Json(serde_json::json!({
            "success": false,
            "error": "Username must be 1-64 characters"
        }));
    }

    let has_users = state
        .repo_blocking(|repo| repo.get_all_users().map(|u| !u.is_empty()).unwrap_or(true))
        .await;
    if has_users {
        return Json(serde_json::json!({
            "success": false,
            "error": "Registration is closed. Ask an admin to create your account."
        }));
    }

    let password_owned = payload.password.clone();
    let password_hash = match tokio::task::spawn_blocking(move || hash(&password_owned, 12)).await {
        Ok(Ok(h)) => h,
        _ => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to hash password"
            }));
        }
    };

    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username,
        password_hash,
        role: Role::Admin,
        created_at: chrono::Utc::now(),
        token_version: 0,
    };

    let user_to_insert = user.clone();
    // Atomically insert only if no users exist (prevents race condition)
    match state
        .repo_blocking(move |repo| repo.insert_first_user(&user_to_insert))
        .await
    {
        Ok(true) => {} // success - first user created
        Ok(false) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Registration is closed. Ask an admin to create your account."
            }));
        }
        Err(e) => {
            tracing::error!("Failed to create initial user: {}", e);
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to create user"
            }));
        }
    }

    let claims = Claims {
        sub: user.username.clone(),
        role: user.role.as_str().to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
        ver: user.token_version,
    };

    let token = match jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(jwt_secret()),
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to encode JWT: {}", e);
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to issue token"
            }));
        }
    };

    Json(serde_json::json!({
        "success": true,
        "token": token,
        "username": user.username,
        "role": user.role.as_str()
    }))
}

async fn login(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Json<serde_json::Value> {
    if let Some(ip) = crate::manager::extract_client_ip(&headers, Some(peer)) {
        if !state.login_limiter.try_consume(ip).await {
            return Json(serde_json::json!({
                "success": false,
                "error": "Too many attempts. Try again later."
            }));
        }
    }
    let username_trimmed = payload.username.trim().to_string();
    if username_trimmed.is_empty() || payload.password.is_empty() {
        return Json(serde_json::json!({
            "success": false,
            "error": "Invalid credentials"
        }));
    }
    // Always run bcrypt even on missing user to equalize timing.
    const DUMMY_HASH: &str = "$2b$12$000000000000000000000uGKWMKFz95uGKWMKFz95uGKWMKFz9.";
    let username_owned = username_trimmed;
    let password_owned = payload.password.clone();
    let auth_result = state
        .repo_blocking(move |repo| {
            let (user, hash_str) = match repo.get_user_by_username(&username_owned) {
                Ok(Some(u)) => {
                    let h = u.password_hash.clone();
                    (Some(u), h)
                }
                _ => (None, DUMMY_HASH.to_string()),
            };
            let valid = verify(&password_owned, &hash_str).unwrap_or(false) && user.is_some();
            if valid {
                user
            } else {
                None
            }
        })
        .await;

    let user = match auth_result {
        Some(u) => u,
        None => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid credentials"
            }));
        }
    };

    let claims = Claims {
        sub: user.username.clone(),
        role: user.role.as_str().to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
        ver: user.token_version,
    };

    let token = match jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(jwt_secret()),
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to encode JWT: {}", e);
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to issue token"
            }));
        }
    };

    Json(serde_json::json!({
        "success": true,
        "token": token,
        "username": user.username,
        "role": user.role.as_str()
    }))
}

async fn verify_token(
    State(state): State<SharedState>,
    Json(token): Json<String>,
) -> Json<serde_json::Value> {
    match state.authenticate(&token).await {
        Ok(user) => Json(serde_json::json!({
            "valid": true,
            "username": user.username,
            "role": user.role.as_str()
        })),
        Err(_) => Json(serde_json::json!({ "valid": false })),
    }
}

// Always 200; token validity is in `success`. app.js relies on this to tell
// "invalid token → drop it" (200 + success:false) from "server down → keep it"
// (request throws) — don't switch this to a 4xx.
async fn get_profile(
    State(state): State<SharedState>,
    axum::extract::Json(token): axum::extract::Json<String>,
) -> Json<serde_json::Value> {
    let user = match state.authenticate(&token).await {
        Ok(u) => u,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    Json(serde_json::json!({
        "success": true,
        "user": {
            "id": user.id,
            "username": user.username,
            "role": user.role.as_str(),
            "created_at": user.created_at.to_rfc3339()
        }
    }))
}

async fn list_users(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if let Err(e) = state.authenticate_session_admin(token).await {
        return Json(serde_json::json!({
            "success": false,
            "error": e
        }));
    }

    let users = state
        .repo_blocking(|repo| repo.get_all_users().unwrap_or_default())
        .await;
    let users_json: Vec<_> = users
        .into_iter()
        .map(|u| {
            serde_json::json!({
                "id": u.id,
                "username": u.username,
                "role": u.role.as_str(),
                "created_at": u.created_at.to_rfc3339()
            })
        })
        .collect();
    Json(serde_json::json!({ "users": users_json }))
}

async fn create_user(
    State(state): State<SharedState>,
    Json(payload): Json<CreateUserRequest>,
) -> Json<serde_json::Value> {
    if let Err(e) = state.authenticate_session_admin(&payload.token).await {
        let error = if e == "Admin access required" {
            "Only admins can create users"
        } else {
            "Invalid token"
        };
        return Json(serde_json::json!({
            "success": false,
            "error": error
        }));
    }

    if payload.password.len() < 4 {
        return Json(serde_json::json!({
            "success": false,
            "error": "Password must be at least 4 characters"
        }));
    }
    let username = payload.username.trim().to_string();
    if username.is_empty() || username.len() > 64 {
        return Json(serde_json::json!({
            "success": false,
            "error": "Username must be 1-64 characters"
        }));
    }

    let username_lookup = username.clone();
    if state
        .repo_blocking(move |repo| repo.get_user_by_username(&username_lookup))
        .await
        .ok()
        .flatten()
        .is_some()
    {
        return Json(serde_json::json!({
            "success": false,
            "error": "Username already exists"
        }));
    }

    let password_owned = payload.password.clone();
    let password_hash = match tokio::task::spawn_blocking(move || hash(&password_owned, 12)).await {
        Ok(Ok(h)) => h,
        _ => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to hash password"
            }));
        }
    };

    let role = match payload.role.as_deref() {
        Some("ADMIN") => Role::Admin,
        _ => Role::User,
    };

    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username,
        password_hash,
        role,
        created_at: chrono::Utc::now(),
        token_version: 0,
    };

    let user_to_insert = user.clone();
    match state
        .repo_blocking(move |repo| repo.insert_user(&user_to_insert))
        .await
    {
        Ok(()) => {}
        Err(crate::db::repository::InsertUserError::UsernameConflict) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Username already exists"
            }));
        }
        Err(e) => {
            tracing::error!("Failed to create user: {}", e);
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to create user"
            }));
        }
    }

    Json(serde_json::json!({
        "success": true,
        "username": user.username,
        "role": user.role.as_str()
    }))
}

async fn change_password(
    State(state): State<SharedState>,
    Json(payload): Json<ChangePasswordRequest>,
) -> Json<serde_json::Value> {
    let user = match state.authenticate_session(&payload.token).await {
        Ok(u) => u,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    if payload.new_password.len() < 4 {
        return Json(serde_json::json!({
            "success": false,
            "error": "Password must be at least 4 characters"
        }));
    }

    let current_password = payload.current_password.clone();
    let stored_hash = user.password_hash.clone();
    let verify_ok = tokio::task::spawn_blocking(move || {
        verify(&current_password, &stored_hash).unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    if !verify_ok {
        return Json(serde_json::json!({
            "success": false,
            "error": "Current password is incorrect"
        }));
    }

    let new_password = payload.new_password.clone();
    let new_hash = match tokio::task::spawn_blocking(move || hash(&new_password, 12)).await {
        Ok(Ok(h)) => h,
        _ => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to hash password"
            }));
        }
    };

    let sub_update = user.username.clone();
    let result = state
        .repo_blocking(move |repo| {
            repo.update_user_password_and_bump_version(&sub_update, &new_hash)
        })
        .await;
    if let Err(e) = result {
        tracing::error!("Failed to update password: {}", e);
        return Json(serde_json::json!({
            "success": false,
            "error": "Failed to update password"
        }));
    }
    // Invalidate any cookie sessions issued before the password change.
    state.sessions.remove_by_username(&user.username).await;
    // Blow the Basic Auth cache so cached old creds stop working immediately.
    state.basic_auth_cache.invalidate_all().await;

    Json(serde_json::json!({
        "success": true,
        "message": "Password changed successfully"
    }))
}

async fn delete_user(
    State(state): State<SharedState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    let actor = match state.authenticate_session_admin(token).await {
        Ok(u) => u,
        Err("Admin access required") => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Only admins can delete users"
            }));
        }
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    if id == actor.id {
        return Json(serde_json::json!({
            "success": false,
            "error": "You cannot delete your own account"
        }));
    }

    let id_for_delete = id.clone();
    let outcome = match state
        .repo_blocking(move |repo| repo.delete_user_guard_last_admin(&id_for_delete))
        .await
    {
        Ok(Some(Some(username))) => username,
        Ok(Some(None)) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "User not found"
            }));
        }
        Ok(None) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Cannot delete the last admin account"
            }));
        }
        Err(e) => {
            tracing::error!("Failed to delete user: {}", e);
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to delete user"
            }));
        }
    };
    state.sessions.remove_by_username(&outcome).await;
    state.basic_auth_cache.invalidate_all().await;

    Json(serde_json::json!({
        "success": true,
        "message": "User deleted successfully"
    }))
}

// ─── API keys ────────────────────────────────────────────────────────────
// Session-only (never an API key), so a leaked key can't mint/list/revoke keys.

async fn list_api_keys(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let user = match state.authenticate_session(bearer_token(&headers)).await {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "success": false, "error": e })),
    };

    let user_id = user.id.clone();
    let keys = state
        .repo_blocking(move |repo| repo.list_api_keys_for_user(&user_id).unwrap_or_default())
        .await;
    let keys_json: Vec<_> = keys
        .into_iter()
        .map(|k| {
            serde_json::json!({
                "id": k.id,
                "name": k.name,
                "prefix": k.prefix,
                "created_at": k.created_at.to_rfc3339(),
                "last_used_at": k.last_used_at.map(|d| d.to_rfc3339()),
            })
        })
        .collect();
    Json(serde_json::json!({ "keys": keys_json }))
}

async fn create_api_key(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CreateApiKeyRequest>,
) -> Json<serde_json::Value> {
    let user = match state.authenticate_session(bearer_token(&headers)).await {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "success": false, "error": e })),
    };

    let name = payload.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return Json(serde_json::json!({
            "success": false,
            "error": "Name must be 1-64 characters"
        }));
    }

    let key = crate::domain::generate_api_key();
    let key_hash = crate::domain::hash_api_key(&key);
    let prefix = key.chars().take(12).collect::<String>();
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();

    let new_key = crate::db::repository::NewApiKey {
        id: id.clone(),
        user_id: user.id.clone(),
        name: name.clone(),
        key_hash,
        prefix: prefix.clone(),
        created_at: created_at.clone(),
    };
    let inserted = state
        .repo_blocking(move |repo| {
            repo.insert_api_key_if_under_cap(&new_key, MAX_API_KEYS_PER_USER)
        })
        .await;
    match inserted {
        Ok(true) => {}
        Ok(false) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Too many API keys (max 50). Revoke some first."
            }));
        }
        Err(e) => {
            tracing::error!("Failed to create API key: {}", e);
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to create API key"
            }));
        }
    }

    Json(serde_json::json!({
        "success": true,
        "id": id,
        "name": name,
        "prefix": prefix,
        "key": key, // shown to the user exactly once
        "created_at": created_at,
    }))
}

async fn delete_api_key(
    State(state): State<SharedState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let user = match state.authenticate_session(bearer_token(&headers)).await {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "success": false, "error": e })),
    };

    let user_id = user.id.clone();
    let removed = state
        .repo_blocking(move |repo| repo.delete_api_key(&id, &user_id).unwrap_or(false))
        .await;
    if removed {
        Json(serde_json::json!({ "success": true }))
    } else {
        Json(serde_json::json!({ "success": false, "error": "API key not found" }))
    }
}
