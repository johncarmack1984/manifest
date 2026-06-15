mod auth;
mod cost;
mod inventory;

use std::sync::Arc;

use aws_sdk_dynamodb::types::AttributeValue;
use axum::{extract::State, routing::get, Json, Router};
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
    pub cfg: Config,
    pub jwks: auth::JwksCache,
}

pub struct Config {
    pub cache_table: String,
    pub cache_ttl: i64,
    pub view_arn: String,
    pub indexed_regions: Vec<String>,
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
            cache_ttl: var("CACHE_TTL_SECONDS").parse().unwrap_or(3600),
            view_arn: var("RESOURCE_EXPLORER_VIEW_ARN"),
            indexed_regions: var("INDEXED_REGIONS")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
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
        jwks: auth::JwksCache::default(),
        cfg: Config::from_env(),
    }));

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/config", get(public_config))
        .route("/api/cost", get(cost::handler))
        .route("/api/inventory", get(inventory::handler))
        .with_state(state);

    run(app).await
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
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
    let body = item.get("body")?.as_s().ok()?;
    serde_json::from_str(body).ok()
}

pub async fn cache_put(s: &AppState, key: &str, v: &Value) {
    let exp = Utc::now().timestamp() + s.0.cfg.cache_ttl;
    let _ = s
        .0
        .ddb
        .put_item()
        .table_name(&s.0.cfg.cache_table)
        .item("cache_key", AttributeValue::S(key.to_string()))
        .item("body", AttributeValue::S(v.to_string()))
        .item("expires_at", AttributeValue::N(exp.to_string()))
        .send()
        .await;
}
