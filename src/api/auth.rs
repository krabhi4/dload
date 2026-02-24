use crate::domain::Claims;
use axum::{
    response::Json,
    routing::post,
    Router,
};
use bcrypt::{hash, verify};
use jsonwebtoken::{decode, encode, Header, EncodingKey, DecodingKey, Validation};

const SECRET: &[u8] = b"dload-secret-key-change-in-production";

pub fn router() -> Router {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/verify", post(verify_token))
}

#[derive(Debug, serde::Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn login(Json(payload): Json<LoginRequest>) -> Json<serde_json::Value> {
    let stored_hash = hash("admin", 10).unwrap();
    
    if payload.username == "admin" && verify(&payload.password, &stored_hash).unwrap_or(false) {
        let claims = Claims {
            sub: payload.username.clone(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
        };
        
        let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET)).unwrap();
        
        Json(serde_json::json!({
            "success": true,
            "token": token,
            "username": payload.username
        }))
    } else {
        Json(serde_json::json!({
            "success": false,
            "error": "Invalid credentials"
        }))
    }
}

async fn verify_token(Json(token): Json<String>) -> Json<serde_json::Value> {
    match decode::<Claims>(&token, &DecodingKey::from_secret(SECRET), &Validation::default()) {
        Ok(_) => Json(serde_json::json!({ "valid": true })),
        Err(_) => Json(serde_json::json!({ "valid": false })),
    }
}
