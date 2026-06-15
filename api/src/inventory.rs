use std::collections::BTreeMap;

use aws_sdk_resourceexplorer2::types::Resource;
use aws_smithy_types::Document;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};

use manifest_api::classify::classify;
use manifest_api::registry::Registry;

use crate::auth::AuthUser;
use crate::{AppState, Refresh, Res};

pub async fn handler(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<Refresh>,
) -> Result<Json<Value>, StatusCode> {
    if !q.requested() {
        if let Some(v) = crate::cache_get(&s, "inventory:v2").await {
            return Ok(Json(v));
        }
    }
    match compute(&s).await {
        Ok(v) => {
            crate::cache_put(&s, "inventory:v2", &v).await;
            Ok(Json(v))
        }
        Err(e) => {
            tracing::error!("inventory compute failed: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

/// Resource Explorer returns tags as the `tags` property (array of {Key, Value}).
fn tags_of(r: &Resource) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(prop) = r.properties().iter().find(|p| p.name() == Some("tags")) else {
        return out;
    };
    let Some(Document::Array(arr)) = prop.data() else {
        return out;
    };
    for item in arr {
        if let Document::Object(map) = item {
            let key = map.get("Key").and_then(doc_str);
            let val = map.get("Value").and_then(doc_str);
            if let (Some(k), Some(v)) = (key, val) {
                out.insert(k, v);
            }
        }
    }
    out
}

fn doc_str(d: &Document) -> Option<String> {
    match d {
        Document::String(s) => Some(s.clone()),
        _ => None,
    }
}

async fn compute(s: &AppState) -> Res<Value> {
    let registry = Registry::load();
    let mut resources: Vec<Value> = Vec::new();

    for region in &s.0.cfg.indexed_regions {
        let query = format!("region:{region}");
        let mut next: Option<String> = None;
        loop {
            let mut req = s.0.re.search().query_string(&query).view_arn(&s.0.cfg.view_arn);
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            let resp = req.send().await?;

            for r in resp.resources() {
                let arn = r.arn().unwrap_or_default().to_string();
                let rtype = r.resource_type().unwrap_or_default().to_string();
                let service = r.service().unwrap_or_default().to_string();
                let name = arn.rsplit(['/', ':']).next().unwrap_or("").to_string();
                let tags = tags_of(r);
                let stack = tags.get("aws:cloudformation:stack-name").map(String::as_str);
                let c = classify(&name, &rtype, &service, stack, &registry);
                resources.push(json!({
                    "arn": arn,
                    "type": rtype,
                    "region": r.region().unwrap_or_default(),
                    "service": service,
                    "name": name,
                    "category": c.category.as_str(),
                    "app": c.app,
                    "protected": c.protected,
                    "reason": c.reason,
                }));
            }

            next = resp.next_token().map(|t| t.to_string());
            if next.is_none() {
                break;
            }
        }
    }

    let mut by_region: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_app: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    for r in &resources {
        *by_region.entry(r["region"].as_str().unwrap_or("").to_string()).or_default() += 1;
        *by_category.entry(r["category"].as_str().unwrap_or("").to_string()).or_default() += 1;
        if let Some(a) = r["app"].as_str() {
            *by_app.entry(a.to_string()).or_default() += 1;
        }
    }

    Ok(json!({
        "count": resources.len(),
        "resources": resources,
        "byRegion": by_region,
        "byApp": by_app,
        "byCategory": by_category,
        "flags": {
            "orphans": by_category.get("orphan").copied().unwrap_or(0),
            "unclaimed": by_category.get("unclaimed").copied().unwrap_or(0),
        },
        "indexedRegions": s.0.cfg.indexed_regions,
        "generatedAt": chrono::Utc::now().to_rfc3339(),
    }))
}
