mod auth;
mod cost;
mod inventory;
mod state;

use std::sync::Arc;

use aws_sdk_dynamodb::types::AttributeValue;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use lambda_http::{run, Error};
use serde_json::{json, Value};

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// `?refresh=1` busts the server-side cache and recomputes.
#[derive(serde::Deserialize, Default)]
pub struct Refresh {
    pub refresh: Option<String>,
}
impl Refresh {
    pub fn requested(&self) -> bool {
        self.refresh.is_some()
    }
}

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub ce: aws_sdk_costexplorer::Client,
    pub re: aws_sdk_resourceexplorer2::Client,
    pub ddb: aws_sdk_dynamodb::Client,
    pub org: aws_sdk_organizations::Client,
    pub acm: aws_sdk_acm::Client,
    /// Base config (this account's creds + region) — used to build assume-role
    /// providers for cross-account inventory of org member accounts.
    pub shared: aws_config::SdkConfig,
    pub cfg: Config,
    pub jwks: auth::JwksCache,
}

pub struct Config {
    pub cache_table: String,
    /// Durable per-resource operator state (overrides + deletion marks).
    pub state_table: String,
    pub cache_ttl: i64,
    pub view_arn: String,
    pub indexed_regions: Vec<String>,
    /// IAM role name assumed in each org member account to inventory it. Empty
    /// disables cross-account inventory (this account only).
    pub member_role: String,
    pub app_url: String,
    pub account_id: String,
    pub cognito_region: String,
    pub cognito_pool_id: String,
    pub cognito_client_id: String,
    pub cognito_hosted_domain: String,
    pub cognito_identity_provider: String,
}

impl Config {
    fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        Config {
            cache_table: var("CACHE_TABLE"),
            state_table: var("STATE_TABLE"),
            cache_ttl: var("CACHE_TTL_SECONDS").parse().unwrap_or(3600),
            view_arn: var("RESOURCE_EXPLORER_VIEW_ARN"),
            indexed_regions: var("INDEXED_REGIONS")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
            member_role: var("MEMBER_INVENTORY_ROLE"),
            app_url: var("APP_URL"),
            account_id: var("ACCOUNT_ID"),
            cognito_region: var("COGNITO_REGION"),
            cognito_pool_id: var("COGNITO_USER_POOL_ID"),
            cognito_client_id: var("COGNITO_CLIENT_ID"),
            cognito_hosted_domain: var("COGNITO_HOSTED_DOMAIN"),
            cognito_identity_provider: var("COGNITO_IDENTITY_PROVIDER"),
        }
    }

    pub fn issuer(&self) -> String {
        format!(
            "https://cognito-idp.{}.amazonaws.com/{}",
            self.cognito_region, self.cognito_pool_id
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .without_time()
        .init();

    let shared = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let state = AppState(Arc::new(Inner {
        ce: aws_sdk_costexplorer::Client::new(&shared),
        re: aws_sdk_resourceexplorer2::Client::new(&shared),
        ddb: aws_sdk_dynamodb::Client::new(&shared),
        org: aws_sdk_organizations::Client::new(&shared),
        acm: aws_sdk_acm::Client::new(&shared),
        jwks: auth::JwksCache::default(),
        cfg: Config::from_env(),
        shared,
    }));

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/config", get(public_config))
        .route("/api/cost", get(cost::handler))
        .route("/api/inventory", get(inventory::handler))
        .route("/api/inventory/classify", post(inventory::reclassify))
        .route("/api/inventory/mark", post(inventory::mark))
        .route("/api/registry/app", post(add_app).put(update_app))
        .with_state(state);

    run(app).await
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

// Add an app to the live project registry (auth required; single-user pool ⇒ owner).
// Persists to DynamoDB so the next inventory load hot-reloads it.
async fn add_app(
    State(s): State<AppState>,
    _u: auth::AuthUser,
    Json(req): Json<manifest_api::registry::NewApp>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    match manifest_api::registry::add_app(&s.0.ddb, &s.0.cfg.cache_table, &req).await {
        Ok(()) => Ok(Json(json!({ "ok": true, "repo": req.repo.trim() }))),
        Err(e) => {
            tracing::warn!("add_app failed: {e}");
            Err((axum::http::StatusCode::UNPROCESSABLE_ENTITY, e))
        }
    }
}

// Update an existing app's registry rules (patterns/types/protected/dead/reason),
// in place — so auto-tagging stays editable after creation.
async fn update_app(
    State(s): State<AppState>,
    _u: auth::AuthUser,
    Json(req): Json<manifest_api::registry::NewApp>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    match manifest_api::registry::update_app(&s.0.ddb, &s.0.cfg.cache_table, &req).await {
        Ok(()) => Ok(Json(json!({ "ok": true, "repo": req.repo.trim() }))),
        Err(e) => {
            tracing::warn!("update_app failed: {e}");
            Err((axum::http::StatusCode::UNPROCESSABLE_ENTITY, e))
        }
    }
}

// Public: lets the SPA bootstrap its Cognito login without a rebuild.
async fn public_config(State(s): State<AppState>) -> Json<Value> {
    let c = &s.0.cfg;
    Json(json!({
        "accountId": c.account_id,
        "appUrl": c.app_url,
        "indexedRegions": c.indexed_regions,
        "cognito": {
            "region": c.cognito_region,
            "userPoolId": c.cognito_pool_id,
            "clientId": c.cognito_client_id,
            "hostedDomain": c.cognito_hosted_domain,
            "identityProvider": c.cognito_identity_provider,
        }
    }))
}

// ---- DynamoDB-backed response cache (Cost Explorer charges $0.01/call) ----
//
// Bodies are stored gzipped: the inventory payload outgrew DynamoDB's 400 KB item
// cap, and an oversized put means the cache silently never hits — every page load
// becomes a full multi-region recompute (plus metered CE calls). Compressed, the
// same payload is a few dozen KB.

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = enc.write_all(bytes);
    enc.finish().unwrap_or_default()
}

fn gunzip(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).ok()?;
    Some(out)
}

pub async fn cache_get(s: &AppState, key: &str) -> Option<Value> {
    let out = s
        .0
        .ddb
        .get_item()
        .table_name(&s.0.cfg.cache_table)
        .key("cache_key", AttributeValue::S(key.to_string()))
        .send()
        .await
        .ok()?;
    let item = out.item?;
    let exp: i64 = item
        .get("expires_at")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    if exp < Utc::now().timestamp() {
        return None;
    }
    if let Some(gz) = item.get("body_gz").and_then(|v| v.as_b().ok()) {
        return serde_json::from_slice(&gunzip(gz.as_ref())?).ok();
    }
    // Plain-JSON items written before compression landed.
    let body = item.get("body")?.as_s().ok()?;
    serde_json::from_str(body).ok()
}

pub async fn cache_put(s: &AppState, key: &str, v: &Value) {
    let exp = Utc::now().timestamp() + s.0.cfg.cache_ttl;
    let gz = gzip(v.to_string().as_bytes());
    let res = s
        .0
        .ddb
        .put_item()
        .table_name(&s.0.cfg.cache_table)
        .item("cache_key", AttributeValue::S(key.to_string()))
        .item("body_gz", AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(gz)))
        .item("expires_at", AttributeValue::N(exp.to_string()))
        .send()
        .await;
    // A failed write must be visible — a silent one turns every request into a
    // recompute and looks like anything but a cache problem from the outside.
    if let Err(e) = res {
        tracing::warn!("cache write for {key} failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::{gunzip, gzip};

    #[test]
    fn oversized_payload_fits_after_compression() {
        // The regression this guards: the inventory JSON crossed DynamoDB's
        // 400 KB item cap, so uncompressed cache writes failed on every compute.
        // A representative >400 KB payload must round-trip and land far under
        // the cap once gzipped.
        let rows: Vec<serde_json::Value> = (0..1500)
            .map(|i| {
                serde_json::json!({
                    "arn": format!("arn:aws:lambda:us-east-1:735853783919:function:app-{i}-fn"),
                    "type": "lambda:function",
                    "region": "us-east-1",
                    "service": "lambda",
                    "name": format!("app-{i}-fn"),
                    "category": "app",
                    "app": format!("app-{i}"),
                    "protected": false,
                    "reason": format!("project 'app-{i}'"),
                    "account": "735853783919",
                    "accountName": "this account",
                })
            })
            .collect();
        let body = serde_json::json!({ "resources": rows }).to_string();
        assert!(body.len() > 400_000, "fixture must exceed the DynamoDB item cap");
        let gz = gzip(body.as_bytes());
        assert!(gz.len() < 100_000, "compressed body should fit with headroom, got {}", gz.len());
        assert_eq!(gunzip(&gz).as_deref(), Some(body.as_bytes()));
    }
}
