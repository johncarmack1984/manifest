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
        if let Some(v) = crate::cache_get(&s, "inventory:v3").await {
            return Ok(Json(v));
        }
    }
    match compute(&s).await {
        Ok(v) => {
            crate::cache_put(&s, "inventory:v3", &v).await;
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
            let mut name = manifest_api::classify::display_name(&arn, &rtype);
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
            // Resource Explorer's index freshness, surfaced as a "last seen" column.
            let last_reported = r
                .last_reported_at()
                .and_then(|t| t.fmt(aws_smithy_types::date_time::Format::DateTime).ok());
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
                "stack": stack,
                "lastReported": last_reported,
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

/// Current-month spend grouped by app, derived from the CloudFormation stack-name
/// cost-allocation tag (each stack → its app via the same registry; unmatched stacks
/// roll up to "unclaimed", resources with no stack to "untagged"). Best-effort: it
/// needs that tag activated in Billing, and CE charges $0.01/call, so it rides the 1h
/// inventory cache. Returns an empty map (no per-app cost shown) on any error.
async fn app_cost(s: &AppState, registry: &Registry) -> std::collections::BTreeMap<String, f64> {
    use aws_sdk_costexplorer::types::{
        DateInterval, Granularity, GroupDefinition, GroupDefinitionType,
    };
    use chrono::Datelike;

    let mut out: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let today = chrono::Utc::now().date_naive();
    let start = today.with_day(1).unwrap_or(today).format("%Y-%m-%d").to_string();
    let end = (today + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    let Ok(interval) = DateInterval::builder().start(start).end(end).build() else {
        return out;
    };

    let resp = s
        .0
        .ce
        .get_cost_and_usage()
        .time_period(interval)
        .granularity(Granularity::Monthly)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Tag)
                .key("aws:cloudformation:stack-name")
                .build(),
        )
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "per-app cost unavailable (activate the aws:cloudformation:stack-name \
                 cost-allocation tag in Billing?): {e}"
            );
            return out;
        }
    };

    for period in resp.results_by_time() {
        for g in period.groups() {
            // For a TAG group the key is the tag value; be defensive about a
            // "tagkey$tagvalue" form too. Empty value ⇒ the resource has no stack.
            let raw = g.keys().first().cloned().unwrap_or_default();
            let stack = raw.rsplit('$').next().unwrap_or(&raw).to_string();
            let amt = g
                .metrics()
                .and_then(|m| m.get("UnblendedCost"))
                .and_then(|v| v.amount())
                .unwrap_or("0")
                .parse::<f64>()
                .unwrap_or(0.0);
            if amt <= 0.0 {
                continue;
            }
            let app = if stack.is_empty() {
                "untagged".to_string()
            } else {
                registry
                    .match_project(&stack, "")
                    .map(|p| p.repo.clone())
                    .unwrap_or_else(|| "unclaimed".to_string())
            };
            *out.entry(app).or_default() += amt;
        }
    }
    out
}

/// Estimated monthly spend per Cost Explorer resource id, from resource-level data
/// (the last ~13 full days, scaled to a 30.44-day month). Opt-in only: it needs
/// "daily granularity resource-level data" enabled in the payer account's Cost
/// Explorer preferences, or this returns empty and the cost column stays blank.
/// Best-effort like `app_cost`, and rides the same inventory cache.
async fn resource_costs(s: &AppState) -> BTreeMap<String, f64> {
    use aws_sdk_costexplorer::types::{
        DateInterval, Dimension, DimensionValues, Expression, Granularity, GroupDefinition,
        GroupDefinitionType,
    };

    // Resource-level retention is 14 days; stay a day inside it.
    const WINDOW_DAYS: i64 = 13;
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    let today = chrono::Utc::now().date_naive();
    let start = (today - chrono::Duration::days(WINDOW_DAYS)).format("%Y-%m-%d").to_string();
    let end = today.format("%Y-%m-%d").to_string();
    let Ok(interval) = DateInterval::builder().start(start).end(end).build() else {
        return out;
    };
    // The API requires a Filter; usage records are also exactly what per-resource
    // cost means (tax/refund/credit lines carry no resource id).
    let usage_only = Expression::builder()
        .dimensions(DimensionValues::builder().key(Dimension::RecordType).values("Usage").build())
        .build();

    let mut next: Option<String> = None;
    loop {
        let mut req = s
            .0
            .ce
            .get_cost_and_usage_with_resources()
            .time_period(interval.clone())
            .granularity(Granularity::Daily)
            .metrics("UnblendedCost")
            .filter(usage_only.clone())
            .group_by(
                GroupDefinition::builder()
                    .r#type(GroupDefinitionType::Dimension)
                    .key("RESOURCE_ID")
                    .build(),
            );
        if let Some(t) = &next {
            req = req.next_page_token(t);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "per-resource cost unavailable (enable daily resource-level data in \
                     the payer account's Cost Explorer preferences?): {e}"
                );
                return BTreeMap::new();
            }
        };
        for period in resp.results_by_time() {
            for g in period.groups() {
                let key = g.keys().first().cloned().unwrap_or_default();
                if key.is_empty() || key == "NoResourceId" {
                    continue;
                }
                let amt = g
                    .metrics()
                    .and_then(|m| m.get("UnblendedCost"))
                    .and_then(|v| v.amount())
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0);
                if amt > 0.0 {
                    *out.entry(key).or_default() += amt;
                }
            }
        }
        next = resp.next_page_token().map(|t| t.to_string());
        if next.is_none() {
            break;
        }
    }

    let scale = 30.44 / WINDOW_DAYS as f64;
    for v in out.values_mut() {
        *v *= scale;
    }
    out
}

/// Attach per-resource cost estimates to inventory rows. Cost Explorer resource ids
/// are full ARNs for some services and bare ids/names for others (EC2 instance ids,
/// S3 bucket names), so try the exact ARN, then the ARN's trailing id, then the
/// display name. Each cost entry lands on at most one resource; a resource billed
/// under several ids accumulates.
fn attach_costs(resources: &mut [Value], costs: BTreeMap<String, f64>) {
    use std::collections::HashMap;
    if costs.is_empty() {
        return;
    }
    let mut by_arn: HashMap<String, usize> = HashMap::new();
    let mut by_tail: HashMap<String, usize> = HashMap::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (i, r) in resources.iter().enumerate() {
        let arn = r["arn"].as_str().unwrap_or_default();
        if arn.is_empty() {
            continue;
        }
        by_arn.entry(arn.to_string()).or_insert(i);
        if let Some(tail) = arn.rsplit(['/', ':']).next().filter(|t| !t.is_empty()) {
            by_tail.entry(tail.to_string()).or_insert(i);
        }
        if let Some(name) = r["name"].as_str().filter(|n| !n.is_empty()) {
            by_name.entry(name.to_string()).or_insert(i);
        }
    }
    for (key, amt) in costs {
        let idx = by_arn.get(&key).or_else(|| by_tail.get(&key)).or_else(|| by_name.get(&key));
        if let Some(&i) = idx {
            let prev = resources[i]["cost"].as_f64().unwrap_or(0.0);
            resources[i]["cost"] = json!(prev + amt);
        }
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

    // 3. Apply durable operator state: a manual classification override wins over
    //    inference, and a deletion mark is surfaced for the UI / reaper.
    let state = crate::state::load(s).await;
    if !state.is_empty() {
        for r in &mut resources {
            let arn = r["arn"].as_str().unwrap_or_default().to_string();
            let Some(st) = state.get(&arn) else {
                continue;
            };
            if let Some(app) = &st.app {
                let proj = registry.projects.iter().find(|p| &p.repo == app);
                r["app"] = json!(app);
                r["category"] = json!(if proj.is_some_and(|p| p.dead) { "orphan" } else { "app" });
                r["protected"] = json!(proj.is_some_and(|p| p.protected));
                r["reason"] = json!(format!("manually assigned to '{app}'"));
                r["override"] = json!(true);
            }
            if let Some(mark) = &st.mark {
                r["mark"] = json!(mark);
            }
        }
    }

    // 4. Per-resource cost estimates (best-effort; blank column when not opted in).
    attach_costs(&mut resources, resource_costs(s).await);

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

    let marked = resources
        .iter()
        .filter(|r| r.get("mark").and_then(|m| m.as_str()).is_some())
        .count();
    let app_cost = app_cost(s, &registry).await;

    // Registry definitions, keyed by app — drives the dashboard's rule editor. On a
    // duplicate repo the first entry wins, matching `match_project` precedence.
    let mut app_meta = serde_json::Map::new();
    for p in &registry.projects {
        app_meta
            .entry(p.repo.clone())
            .or_insert_with(|| serde_json::to_value(p).unwrap_or(Value::Null));
    }

    Ok(json!({
        "count": resources.len(),
        "resources": resources,
        "byRegion": by_region,
        "byApp": by_app,
        "byCategory": by_category,
        "byAccount": by_account,
        "byAppCost": app_cost,
        "apps": registry.projects.iter().map(|p| p.repo.as_str()).collect::<std::collections::BTreeSet<_>>(),
        "appMeta": app_meta,
        "flags": {
            "orphans": by_category.get("orphan").copied().unwrap_or(0),
            "unclaimed": by_category.get("unclaimed").copied().unwrap_or(0),
            "marked": marked,
            "notIndexed": not_indexed,
        },
        "indexedRegions": s.0.cfg.indexed_regions,
        "generatedAt": chrono::Utc::now().to_rfc3339(),
    }))
}

#[derive(serde::Deserialize)]
pub struct ReclassifyReq {
    /// Resources to (re)attribute, by ARN.
    pub arns: Vec<String>,
    /// Target app. Null or empty clears the override (back to inferred classification).
    #[serde(default)]
    pub app: Option<String>,
}

/// Manually (re)classify resources into an app — fixes misattributed or unclaimed
/// resources, and is the tool that drives "unclaimed" toward zero. Writes a per-ARN
/// override to the state table; the next inventory load reflects it. Auth required
/// (the pool has a single user, so any authenticated caller is the owner).
pub async fn reclassify(
    State(s): State<AppState>,
    _u: AuthUser,
    Json(req): Json<ReclassifyReq>,
) -> Result<Json<Value>, StatusCode> {
    if s.0.cfg.state_table.is_empty() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let app = req.app.as_deref().map(str::trim).filter(|a| !a.is_empty());
    for arn in &req.arns {
        if let Err(e) = crate::state::set_override(&s, arn, app).await {
            tracing::error!("reclassify {arn} failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    }
    Ok(Json(json!({ "ok": true, "count": req.arns.len(), "app": app })))
}

#[derive(serde::Deserialize)]
pub struct MarkReq {
    /// Resources to (un)mark, by ARN.
    pub arns: Vec<String>,
    /// True to flag for deletion, false to clear the flag.
    pub marked: bool,
}

/// Flag resources for deletion (or clear the flag). This only records intent in the
/// state table — nothing is destroyed here. The operator-run reap tool consumes the
/// marks and performs the deletion with its own credentials.
pub async fn mark(
    State(s): State<AppState>,
    _u: AuthUser,
    Json(req): Json<MarkReq>,
) -> Result<Json<Value>, StatusCode> {
    if s.0.cfg.state_table.is_empty() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    for arn in &req.arns {
        if let Err(e) = crate::state::set_mark(&s, arn, req.marked).await {
            tracing::error!("mark {arn} failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    }
    Ok(Json(json!({ "ok": true, "count": req.arns.len(), "marked": req.marked })))
}
