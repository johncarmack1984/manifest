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

/// The regions to sweep for one account: the configured billing regions plus the
/// pseudo-region "global", where Resource Explorer files CloudFront/Route53/IAM/WAF
/// (the indexed regions never return those).
fn scan_regions(cfg_regions: &[String]) -> Vec<String> {
    cfg_regions
        .iter()
        .cloned()
        .chain(std::iter::once("global".to_string()))
        .collect()
}

/// Search one region (or "global") through `re`, classify each resource, and append
/// it to `out` stamped with its owning account. `view_arn` is the manifest-owned
/// aggregator view for the local account; member accounts pass `None` to use that
/// region's default view.
async fn scan_region(
    re: &aws_sdk_resourceexplorer2::Client,
    acm: &aws_sdk_acm::Client,
    view_arn: Option<&str>,
    region: &str,
    registry: &Registry,
    account_id: &str,
    account_name: &str,
    out: &mut Vec<Value>,
) -> Res<()> {
    let query = format!("region:{region}");
    let mut next: Option<String> = None;
    loop {
        let mut req = re.search().query_string(&query);
        if let Some(v) = view_arn {
            req = req.view_arn(v);
        }
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req.send().await?;

        for r in resp.resources() {
            let arn = r.arn().unwrap_or_default().to_string();
            let rtype = r.resource_type().unwrap_or_default().to_string();
            let service = r.service().unwrap_or_default().to_string();
            let mut name = arn.rsplit(['/', ':']).next().unwrap_or("").to_string();
            // ACM certs are UUID-named in Resource Explorer; surface the domain
            // instead — both as the display name and so it classifies by project.
            if rtype == "acm:certificate" {
                if let Ok(out) = acm.describe_certificate().certificate_arn(&arn).send().await {
                    if let Some(d) = out.certificate().and_then(|c| c.domain_name()) {
                        name = d.to_string();
                    }
                }
            }
            let tags = tags_of(r);
            let stack = tags.get("aws:cloudformation:stack-name").map(String::as_str);
            let c = classify(&name, &rtype, &service, stack, registry);
            out.push(json!({
                "arn": arn,
                "type": rtype,
                "region": r.region().unwrap_or_default(),
                "service": service,
                "name": name,
                "category": c.category.as_str(),
                "app": c.app,
                "protected": c.protected,
                "reason": c.reason,
                "account": account_id,
                "accountName": account_name,
            }));
        }

        next = resp.next_token().map(|t| t.to_string());
        if next.is_none() {
            break;
        }
    }
    Ok(())
}

/// Active org member accounts other than this one. Errors (e.g. when not running in
/// the management / delegated-admin account) leave the caller showing just this account.
async fn list_member_accounts(
    org: &aws_sdk_organizations::Client,
    self_id: &str,
) -> Res<Vec<(String, String)>> {
    use aws_sdk_organizations::types::AccountStatus;
    let mut out = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = org.list_accounts();
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req.send().await?;
        for a in resp.accounts() {
            let id = a.id().unwrap_or_default().to_string();
            if id.is_empty() || id == self_id || a.status() != Some(&AccountStatus::Active) {
                continue;
            }
            let name = a.name().map(String::from).unwrap_or_else(|| id.clone());
            out.push((id, name));
        }
        next = resp.next_token().map(|t| t.to_string());
        if next.is_none() {
            break;
        }
    }
    Ok(out)
}

/// Inventory one member account: assume its read role, then sweep each region with
/// credentials scoped to that account. Member accounts have no shared aggregator, so
/// every region is queried against its own index + default view; regions without an
/// index simply error and are skipped. Returns Err only if no region was queryable
/// (assume-role denied, or Resource Explorer not enabled), so the account is flagged.
async fn scan_member(
    s: &AppState,
    registry: &Registry,
    regions: &[String],
    account_id: &str,
    account_name: &str,
    out: &mut Vec<Value>,
) -> Res<()> {
    let role_arn = format!("arn:aws:iam::{account_id}:role/{}", s.0.cfg.member_role);
    let provider = aws_config::sts::AssumeRoleProvider::builder(role_arn)
        .session_name("manifest-inventory")
        .configure(&s.0.shared)
        .build()
        .await;
    // One SdkConfig holds the assumed credentials (a single STS call, then cached);
    // per-region clients are derived from it with the region overridden.
    let member_cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .credentials_provider(provider)
        .region(aws_config::Region::new("us-east-1"))
        .load()
        .await;

    let mut queried_any = false;
    let mut last_err: Option<String> = None;
    for region in regions {
        // "global" is a Resource Explorer filter value, not an endpoint — query it
        // against a real regional endpoint (members have no aggregator to fall back on).
        let endpoint = if region == "global" { "us-east-1" } else { region.as_str() };
        let r = aws_config::Region::new(endpoint.to_string());
        let re = aws_sdk_resourceexplorer2::Client::from_conf(
            aws_sdk_resourceexplorer2::config::Builder::from(&member_cfg).region(r.clone()).build(),
        );
        let acm = aws_sdk_acm::Client::from_conf(
            aws_sdk_acm::config::Builder::from(&member_cfg).region(r).build(),
        );
        match scan_region(&re, &acm, None, region, registry, account_id, account_name, out).await {
            Ok(()) => queried_any = true,
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    if queried_any {
        Ok(())
    } else {
        Err(last_err.unwrap_or_else(|| "no Resource Explorer index reachable".into()).into())
    }
}

async fn compute(s: &AppState) -> Res<Value> {
    let registry = Registry::from_dynamo(&s.0.ddb, &s.0.cfg.cache_table).await;
    let regions = scan_regions(&s.0.cfg.indexed_regions);
    let mut resources: Vec<Value> = Vec::new();
    // Accounts we tried but couldn't reach — surfaced so a member that's silently
    // missing from inventory is visible rather than looking like it has no resources.
    let mut not_indexed: Vec<Value> = Vec::new();

    // 1. The account manifest runs in, via its own manifest-owned aggregator view.
    for region in &regions {
        scan_region(
            &s.0.re,
            &s.0.acm,
            Some(&s.0.cfg.view_arn),
            region,
            &registry,
            &s.0.cfg.account_id,
            "this account",
            &mut resources,
        )
        .await?;
    }

    // 2. Other org member accounts. Inventory (unlike org-wide cost) is per-account,
    //    so each needs an assumed read role; configure MEMBER_INVENTORY_ROLE to enable.
    if !s.0.cfg.member_role.is_empty() {
        match list_member_accounts(&s.0.org, &s.0.cfg.account_id).await {
            Ok(accounts) => {
                for (id, name) in accounts {
                    if let Err(e) =
                        scan_member(s, &registry, &regions, &id, &name, &mut resources).await
                    {
                        tracing::warn!("member account {id} ({name}) not indexed: {e}");
                        not_indexed.push(json!({
                            "account": id, "accountName": name, "reason": e.to_string(),
                        }));
                    }
                }
            }
            Err(e) => tracing::info!("not enumerating member accounts ({e}); this account only"),
        }
    }

    let mut by_region: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_app: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_account: BTreeMap<String, usize> = BTreeMap::new();
    for r in &resources {
        *by_region.entry(r["region"].as_str().unwrap_or("").to_string()).or_default() += 1;
        *by_category.entry(r["category"].as_str().unwrap_or("").to_string()).or_default() += 1;
        if let Some(a) = r["app"].as_str() {
            *by_app.entry(a.to_string()).or_default() += 1;
        }
        let acct = r["accountName"]
            .as_str()
            .filter(|a| !a.is_empty())
            .or_else(|| r["account"].as_str())
            .unwrap_or("")
            .to_string();
        *by_account.entry(acct).or_default() += 1;
    }

    Ok(json!({
        "count": resources.len(),
        "resources": resources,
        "byRegion": by_region,
        "byApp": by_app,
        "byCategory": by_category,
        "byAccount": by_account,
        "flags": {
            "orphans": by_category.get("orphan").copied().unwrap_or(0),
            "unclaimed": by_category.get("unclaimed").copied().unwrap_or(0),
            "notIndexed": not_indexed,
        },
        "indexedRegions": s.0.cfg.indexed_regions,
        "generatedAt": chrono::Utc::now().to_rfc3339(),
    }))
}
