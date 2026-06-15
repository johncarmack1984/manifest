use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::{request::Parts, StatusCode};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::AppState;

#[derive(Default, Clone)]
pub struct JwksCache(Arc<RwLock<HashMap<String, DecodingKey>>>);

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields are required for deserialization / JWT validation
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    pub token_use: String,
    pub exp: usize,
}

/// Authenticated principal — used as an extractor on protected routes.
pub struct AuthUser(#[allow(dead_code)] pub Claims);

async fn key_for(state: &AppState, kid: &str) -> Option<DecodingKey> {
    if let Some(k) = state.0.jwks.0.read().await.get(kid).cloned() {
        return Some(k);
    }
    let url = format!(
        "https://cognito-idp.{}.amazonaws.com/{}/.well-known/jwks.json",
        state.0.cfg.cognito_region, state.0.cfg.cognito_pool_id
    );
    let jwks: Jwks = reqwest::get(&url).await.ok()?.json().await.ok()?;
    let mut w = state.0.jwks.0.write().await;
    for jwk in jwks.keys {
        if let Ok(dk) = DecodingKey::from_rsa_components(&jwk.n, &jwk.e) {
            w.insert(jwk.kid, dk);
        }
    }
    w.get(kid).cloned()
}

pub async fn verify(state: &AppState, token: &str) -> Result<Claims, ()> {
    let header = decode_header(token).map_err(|_| ())?;
    let kid = header.kid.ok_or(())?;
    let key = key_for(state, &kid).await.ok_or(())?;

    let mut v = Validation::new(Algorithm::RS256);
    v.set_audience(&[state.0.cfg.cognito_client_id.as_str()]);
    v.set_issuer(&[state.0.cfg.issuer().as_str()]);

    let data = decode::<Claims>(token, &key, &v).map_err(|_| ())?;
    if data.claims.token_use != "id" {
        return Err(());
    }
    Ok(data.claims)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        // CloudFront's Lambda OAC signs with SigV4 in the Authorization header,
        // so the Cognito ID token travels in X-Id-Token instead.
        let token = parts
            .headers
            .get("x-id-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if token.is_empty() {
            return Err(StatusCode::UNAUTHORIZED);
        }
        match verify(state, token).await {
            Ok(c) => Ok(AuthUser(c)),
            Err(()) => Err(StatusCode::UNAUTHORIZED),
        }
    }
}
