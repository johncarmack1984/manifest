use aws_sdk_resourceexplorer2::types::Resource;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::auth::AuthUser;
use crate::{AppState, Refresh, Res};

/// Resource types that are AWS-managed defaults, plumbing, or version artifacts —
/// the noise that swamps an account inventory. Hidden by default in the UI.
const MANAGED_TYPES: &[&str] = &[
    "lambda:function/version",
    "ec2:security-group-rule",
    "ec2:subnet",
    "ec2:route-table",
    "ec2:network-acl",
    "ec2:internet-gateway",
    "ec2:dhcp-options",
    "memorydb:parametergroup",
    "memorydb:user",
    "memorydb:acl",
    "memorydb:subnetgroup",
    "elasticache:user",
    "rds:pg",
    "rds:og",
    "rds:secgrp",
    "rds:subgrp",
    "athena:datacatalog",
    "athena:workgroup",
    "xray:sampling-rule",
    "events:event-bus",
    "resource-explorer-2:index",
];

pub async fn handler(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<Refresh>,
) -> Result<Json<Value>, StatusCode> {
    if !q.requested() {
        if let Some(v) = crate::cache_get(&s, "inventory:v1").await {
            return Ok(Json(v));
        }
    }
    match compute(&s).await {
        Ok(v) => {
            crate::cache_put(&s, "inventory:v1", &v).await;
            Ok(Json(v))
        }
        Err(e) => {
            tracing::error!("inventory compute failed: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

fn has_tags(r: &Resource) -> bool {
    r.properties().iter().any(|p| p.name() == Some("tags"))
}

fn is_managed(rtype: &str, name: &str) -> bool {
    MANAGED_TYPES.contains(&rtype) || name.starts_with("default") || name == "AwsDataCatalog"
}

async fn compute(s: &AppState) -> Res<Value> {
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
                let name = arn.rsplit([':', '/']).next().unwrap_or("").to_string();
                let untagged = !has_tags(r);
                let managed = is_managed(&rtype, &name);
                resources.push(json!({
                    "arn": arn,
                    "type": rtype,
                    "region": r.region().unwrap_or_default(),
                    "service": r.service().unwrap_or_default(),
                    "name": name,
                    "untagged": untagged,
                    "managed": managed,
                }));
            }

            next = resp.next_token().map(|t| t.to_string());
            if next.is_none() {
                break;
            }
        }
    }

    let mut by_region: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut owned = 0usize;
    let mut untagged_total = 0usize;
    let mut untagged_owned = 0usize;
    for r in &resources {
        *by_region.entry(r["region"].as_str().unwrap_or("").to_string()).or_default() += 1;
        *by_type.entry(r["type"].as_str().unwrap_or("").to_string()).or_default() += 1;
        let managed = r["managed"].as_bool().unwrap_or(false);
        let untagged = r["untagged"].as_bool().unwrap_or(false);
        if untagged {
            untagged_total += 1;
        }
        if !managed {
            owned += 1;
            if untagged {
                untagged_owned += 1;
            }
        }
    }

    Ok(json!({
        "count": resources.len(),
        "ownedCount": owned,
        "resources": resources,
        "byRegion": by_region,
        "byType": by_type,
        "flags": { "untaggedCount": untagged_total, "untaggedOwned": untagged_owned },
        "indexedRegions": s.0.cfg.indexed_regions,
        "generatedAt": chrono::Utc::now().to_rfc3339(),
    }))
}
