use crate::domain::{Claims, Role, User, JWT_SECRET};
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
        .route("/api/auth/users/:id", delete(delete_user))
        .with_state(state)
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

fn decode_token(token: &str) -> Result<Claims, String> {
    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(JWT_SECRET),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Invalid token".to_string())
}

async fn auth_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let has_users = state.repo.get_all_users().map(|u| !u.is_empty()).unwrap_or(false);
    Json(serde_json::json!({
        "needs_setup": !has_users
    }))
}

async fn register(
    State(state): State<SharedState>,
    Json(payload): Json<RegisterRequest>,
) -> Json<serde_json::Value> {
    let repo = &state.repo;

    let password_hash = match hash(&payload.password, 10) {
        Ok(h) => h,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to hash password"
            }));
        }
    };

    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username: payload.username.clone(),
        password_hash,
        role: Role::Admin,
        created_at: chrono::Utc::now(),
    };

    // Atomically insert only if no users exist (prevents race condition)
    match repo.insert_first_user(&user) {
        Ok(true) => {} // success - first user created
        Ok(false) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Registration is closed. Ask an admin to create your account."
            }));
        }
        Err(e) => {
            return Json(serde_json::json!({
                "success": false,
                "error": format!("Failed to create user: {}", e)
            }));
        }
    }

    let claims = Claims {
        sub: user.username.clone(),
        role: user.role.as_str().to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
    };

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET),
    )
    .unwrap();

    Json(serde_json::json!({
        "success": true,
        "token": token,
        "username": user.username,
        "role": user.role.as_str()
    }))
}

async fn login(
    State(state): State<SharedState>,
    Json(payload): Json<LoginRequest>,
) -> Json<serde_json::Value> {
    let repo = &state.repo;
    
    let user = match repo.get_user_by_username(&payload.username) {
        Ok(Some(u)) => u,
        Ok(None) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid credentials"
            }));
        }
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Database error"
            }));
        }
    };

    if !verify(&payload.password, &user.password_hash).unwrap_or(false) {
        return Json(serde_json::json!({
            "success": false,
            "error": "Invalid credentials"
        }));
    }

    let claims = Claims {
        sub: user.username.clone(),
        role: user.role.as_str().to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
    };

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET),
    )
    .unwrap();

    Json(serde_json::json!({
        "success": true,
        "token": token,
        "username": user.username,
        "role": user.role.as_str()
    }))
}

async fn verify_token(Json(token): Json<String>) -> Json<serde_json::Value> {
    match jsonwebtoken::decode::<Claims>(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(JWT_SECRET),
        &jsonwebtoken::Validation::default(),
    ) {
        Ok(decoded) => Json(serde_json::json!({
            "valid": true,
            "username": decoded.claims.sub,
            "role": decoded.claims.role
        })),
        Err(_) => Json(serde_json::json!({ "valid": false })),
    }
}

async fn get_profile(
    State(state): State<SharedState>,
    axum::extract::Json(token): axum::extract::Json<String>,
) -> Json<serde_json::Value> {
    let claims = match decode_token(&token) {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    let user = match state.repo.get_user_by_username(&claims.sub) {
        Ok(Some(u)) => u,
        _ => {
            return Json(serde_json::json!({
                "success": false,
                "error": "User not found"
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

    let claims = match decode_token(token) {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Authentication required"
            }));
        }
    };

    if claims.role != "ADMIN" {
        return Json(serde_json::json!({
            "success": false,
            "error": "Admin access required"
        }));
    }

    let users = state.repo.get_all_users().unwrap_or_default();
    let users_json: Vec<_> = users
        .into_iter()
        .map(|u| serde_json::json!({
            "id": u.id,
            "username": u.username,
            "role": u.role.as_str(),
            "created_at": u.created_at.to_rfc3339()
        }))
        .collect();
    Json(serde_json::json!({ "users": users_json }))
}

async fn create_user(
    State(state): State<SharedState>,
    Json(payload): Json<CreateUserRequest>,
) -> Json<serde_json::Value> {
    let claims = match decode_token(&payload.token) {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    if claims.role != "ADMIN" {
        return Json(serde_json::json!({
            "success": false,
            "error": "Only admins can create users"
        }));
    }

    if state.repo.get_user_by_username(&payload.username).ok().flatten().is_some() {
        return Json(serde_json::json!({
            "success": false,
            "error": "Username already exists"
        }));
    }

    let password_hash = match hash(&payload.password, 10) {
        Ok(h) => h,
        Err(_) => {
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
        username: payload.username.clone(),
        password_hash,
        role,
        created_at: chrono::Utc::now(),
    };

    if let Err(e) = state.repo.insert_user(&user) {
        return Json(serde_json::json!({
            "success": false,
            "error": format!("Failed to create user: {}", e)
        }));
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
    let claims = match decode_token(&payload.token) {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    let user = match state.repo.get_user_by_username(&claims.sub) {
        Ok(Some(u)) => u,
        _ => {
            return Json(serde_json::json!({
                "success": false,
                "error": "User not found"
            }));
        }
    };

    if !verify(&payload.current_password, &user.password_hash).unwrap_or(false) {
        return Json(serde_json::json!({
            "success": false,
            "error": "Current password is incorrect"
        }));
    }

    let new_hash = match hash(&payload.new_password, 10) {
        Ok(h) => h,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Failed to hash password"
            }));
        }
    };

    if let Err(e) = state.repo.update_user_password(&claims.sub, &new_hash) {
        return Json(serde_json::json!({
            "success": false,
            "error": format!("Failed to update password: {}", e)
        }));
    }

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

    let claims = match decode_token(token) {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "error": "Invalid token"
            }));
        }
    };

    if claims.role != "ADMIN" {
        return Json(serde_json::json!({
            "success": false,
            "error": "Only admins can delete users"
        }));
    }

    // Prevent self-deletion
    if let Ok(Some(target_user)) = state.repo.get_user_by_id(&id) {
        if target_user.username == claims.sub {
            return Json(serde_json::json!({
                "success": false,
                "error": "You cannot delete your own account"
            }));
        }
    }

    if let Err(e) = state.repo.delete_user(&id) {
        return Json(serde_json::json!({
            "success": false,
            "error": format!("Failed to delete user: {}", e)
        }));
    }

    Json(serde_json::json!({
        "success": true,
        "message": "User deleted successfully"
    }))
}
