//! Local operator tool: delete the resources you flagged "mark for deletion" in the
//! dashboard. Reads the marks from the `manifest-state` table, cross-references the
//! live inventory (Resource Explorer), and deletes the standalone, non-IaC ones with
//! YOUR credentials — deliberately NOT the Lambda role, so the hosted dashboard never
//! holds delete permissions.
//!
//! SAFE BY DEFAULT:
//!   - Dry run unless you pass `--apply`. `--yes` skips the per-resource confirm.
//!   - REFUSES CloudFormation/CDK stack members (deleting a leaf just causes drift —
//!     destroy the stack via its IaC instead) and anything classified `protected`.
//!   - Only deletes a curated set of types; everything else is reported, not touched.
//!
//! Env (sourced from infra/.env by `just reap`): MANIFEST_RESOURCE_EXPLORER_VIEW_ARN,
//! MANIFEST_INDEXED_REGIONS, and STATE_TABLE (defaults to `<MANIFEST_NAME>-state`).

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::process::Command;

use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_resourceexplorer2::types::Resource;
use aws_smithy_types::Document;
use manifest_api::classify::classify;
use manifest_api::registry::Registry;

struct Row {
    arn: String,
    rtype: String,
    region: String,
    name: String,
    protected: bool,
    /// Owning CloudFormation/CDK stack, if any (from the aws:cloudformation:stack-name tag).
    stack: Option<String>,
}

enum Disposition {
    /// Standalone, supported type — safe for the reaper to delete.
    Delete,
    /// Owned by a CloudFormation/CDK stack; destroy the stack via its IaC instead.
    RefuseStack(String),
    /// Classified protected — never deleted.
    RefuseProtected,
    /// Not a type the reaper knows how to delete.
    Unsupported,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let apply = args.iter().any(|a| a == "--apply");
    let yes = args.iter().any(|a| a == "--yes");

    let shared = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let re = aws_sdk_resourceexplorer2::Client::new(&shared);
    let ddb = aws_sdk_dynamodb::Client::new(&shared);

    let cache_table = std::env::var("CACHE_TABLE")
        .unwrap_or_else(|_| format!("{}-cache", std::env::var("MANIFEST_NAME").unwrap_or_else(|_| "manifest".into())));
    // A DELETER must classify against the LIVE registry — the embedded example doesn't
    // know what's protected, so silently falling back to it would disable the
    // protected-resource guard (the bug that let a protected bucket be deleted). Refuse
    // to run without it.
    let reg = match Registry::try_from_dynamo(&ddb, &cache_table).await {
        Some(r) => r,
        None => {
            eprintln!(
                "refusing to reap: couldn't load the live registry from DynamoDB ({cache_table}). \
                 Run `just registry-push` first so the protected-resource guard is accurate."
            );
            std::process::exit(1);
        }
    };

    let view_arn = std::env::var("MANIFEST_RESOURCE_EXPLORER_VIEW_ARN").unwrap_or_default();
    if view_arn.is_empty() {
        eprintln!(
            "warning: MANIFEST_RESOURCE_EXPLORER_VIEW_ARN unset — the scan may miss regions \
             other than us-east-1. Set it to the ResourceExplorerViewArn stack output for full coverage.\n"
        );
    }
    let state_table = std::env::var("STATE_TABLE").unwrap_or_else(|_| {
        format!("{}-state", std::env::var("MANIFEST_NAME").unwrap_or_else(|_| "manifest".into()))
    });
    // Indexed regions plus the "global" pseudo-region (IAM/CloudFront/Route53 live there).
    let mut regions: Vec<String> = std::env::var("MANIFEST_INDEXED_REGIONS")
        .unwrap_or_else(|_| "us-east-1".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    regions.push("global".into());

    // 1. The set of ARNs flagged for deletion in the dashboard.
    let marked = load_marked(&ddb, &state_table).await?;
    if marked.is_empty() {
        println!("Nothing is marked for deletion. (Mark resources in the dashboard, then re-run.)");
        return Ok(());
    }

    // 2. The live inventory, so each marked ARN gets its type/stack/protected status.
    let mut rows: Vec<Row> = Vec::new();
    for region in &regions {
        let query = format!("region:{region}");
        let mut next: Option<String> = None;
        loop {
            let mut req = re.search().query_string(&query);
            if !view_arn.is_empty() {
                req = req.view_arn(&view_arn);
            }
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            let resp = req.send().await?;
            for r in resp.resources() {
                let row = row_from(r, &reg);
                if marked.contains(&row.arn) {
                    rows.push(row);
                }
            }
            next = resp.next_token().map(|t| t.to_string());
            if next.is_none() {
                break;
            }
        }
    }

    // 3. Decide what to do with each, and report.
    let found: HashSet<&str> = rows.iter().map(|r| r.arn.as_str()).collect();
    let missing: Vec<&String> = marked.iter().filter(|a| !found.contains(a.as_str())).collect();

    let mut to_delete: Vec<&Row> = Vec::new();
    let mut refused: Vec<(&Row, String)> = Vec::new();
    let mut unsupported: Vec<&Row> = Vec::new();
    for r in &rows {
        match disposition(r) {
            Disposition::Delete => to_delete.push(r),
            Disposition::RefuseStack(s) => refused.push((r, format!("owned by stack '{s}' — destroy it via its IaC"))),
            Disposition::RefuseProtected => refused.push((r, "protected".into())),
            Disposition::Unsupported => unsupported.push(r),
        }
    }

    println!("== marked for deletion: {} ==\n", marked.len());
    if !to_delete.is_empty() {
        println!("WILL DELETE ({}):", to_delete.len());
        for r in &to_delete {
            println!("  [{:<10}] {:<50} {}", r.region, r.name, r.rtype);
        }
        println!();
    }
    if !refused.is_empty() {
        println!("REFUSED ({}) — handle these yourself:", refused.len());
        for (r, why) in &refused {
            println!("  [{:<10}] {:<50} {:<24} — {why}", r.region, r.name, r.rtype);
        }
        println!();
    }
    if !unsupported.is_empty() {
        println!("SKIPPED ({}) — the reaper can't delete these types; use the console:", unsupported.len());
        for r in &unsupported {
            println!("  [{:<10}] {:<50} {}", r.region, r.name, r.rtype);
        }
        println!();
    }
    if !missing.is_empty() {
        println!("STALE MARKS ({}) — marked but no longer in inventory (already gone?):", missing.len());
        for a in &missing {
            println!("  {a}");
        }
        println!();
    }

    if to_delete.is_empty() {
        println!("Nothing for the reaper to delete.");
        return Ok(());
    }
    if !apply {
        println!("Dry run — nothing deleted. Re-run `just reap --apply` to delete the {} above (`--yes` to skip confirms).", to_delete.len());
        return Ok(());
    }

    // 4. Apply: delete each, confirming per resource unless --yes.
    let mut deleted = 0usize;
    let mut failed = 0usize;
    for r in to_delete {
        if !yes && !confirm(&format!("Delete {} ({}) in {}?", r.name, r.rtype, r.region)) {
            println!("  skipped");
            continue;
        }
        match delete_resource(&r.rtype, &r.arn, &r.region, &r.name) {
            Ok(()) => {
                println!("  ✓ deleted {}", r.name);
                let _ = clear_mark(&ddb, &state_table, &r.arn).await;
                deleted += 1;
            }
            Err(e) => {
                println!("  ✗ {}: {e}", r.name);
                failed += 1;
            }
        }
    }
    println!("\nDeleted {deleted}; {failed} failed. (Failed/refused/skipped resources keep their mark.)");
    Ok(())
}

fn disposition(r: &Row) -> Disposition {
    if r.protected {
        return Disposition::RefuseProtected;
    }
    if let Some(s) = &r.stack {
        return Disposition::RefuseStack(s.clone());
    }
    if supported(&r.rtype) {
        Disposition::Delete
    } else {
        Disposition::Unsupported
    }
}

/// Types the reaper knows how to fully delete (with their dependencies).
fn supported(rtype: &str) -> bool {
    matches!(
        rtype,
        "iam:role"
            | "iam:user"
            | "iam:policy"
            | "lambda:function"
            | "logs:log-group"
            | "dynamodb:table"
            | "sns:topic"
            | "ses:identity"
            | "cloudwatch:alarm"
            | "s3:bucket"
    )
}

/// Run the right `aws` CLI command(s) to delete one resource. Shelling out keeps the
/// actions transparent and avoids pulling a dozen delete-only SDKs into the build.
fn delete_resource(rtype: &str, arn: &str, region: &str, name: &str) -> Result<(), String> {
    match rtype {
        "lambda:function" => run(&["lambda", "delete-function", "--region", region, "--function-name", name]),
        "logs:log-group" => run(&["logs", "delete-log-group", "--region", region, "--log-group-name", &log_group_name(arn)]),
        "dynamodb:table" => run(&["dynamodb", "delete-table", "--region", region, "--table-name", name]),
        "sns:topic" => run(&["sns", "delete-topic", "--region", region, "--topic-arn", arn]),
        "ses:identity" => run(&["sesv2", "delete-email-identity", "--region", region, "--email-identity", name]),
        "cloudwatch:alarm" => run(&["cloudwatch", "delete-alarms", "--region", region, "--alarm-names", name]),
        "s3:bucket" => run(&["s3", "rb", &format!("s3://{name}"), "--force"]),
        "iam:role" => delete_iam_role(name),
        "iam:user" => delete_iam_user(name),
        "iam:policy" => delete_iam_policy(arn),
        other => Err(format!("unsupported type {other}")),
    }
}

// ---- IAM deletes: detach dependencies first, or the delete is rejected. ----

fn delete_iam_role(name: &str) -> Result<(), String> {
    for p in aws_json(&["iam", "list-attached-role-policies", "--role-name", name])["AttachedPolicies"]
        .as_array().into_iter().flatten()
    {
        if let Some(arn) = p["PolicyArn"].as_str() {
            run(&["iam", "detach-role-policy", "--role-name", name, "--policy-arn", arn])?;
        }
    }
    for p in aws_json(&["iam", "list-role-policies", "--role-name", name])["PolicyNames"]
        .as_array().into_iter().flatten()
    {
        if let Some(pn) = p.as_str() {
            run(&["iam", "delete-role-policy", "--role-name", name, "--policy-name", pn])?;
        }
    }
    for ip in aws_json(&["iam", "list-instance-profiles-for-role", "--role-name", name])["InstanceProfiles"]
        .as_array().into_iter().flatten()
    {
        if let Some(ipn) = ip["InstanceProfileName"].as_str() {
            run(&["iam", "remove-role-from-instance-profile", "--instance-profile-name", ipn, "--role-name", name])?;
        }
    }
    run(&["iam", "delete-role", "--role-name", name])
}

fn delete_iam_user(name: &str) -> Result<(), String> {
    for k in aws_json(&["iam", "list-access-keys", "--user-name", name])["AccessKeyMetadata"]
        .as_array().into_iter().flatten()
    {
        if let Some(id) = k["AccessKeyId"].as_str() {
            run(&["iam", "delete-access-key", "--user-name", name, "--access-key-id", id])?;
        }
    }
    for p in aws_json(&["iam", "list-attached-user-policies", "--user-name", name])["AttachedPolicies"]
        .as_array().into_iter().flatten()
    {
        if let Some(arn) = p["PolicyArn"].as_str() {
            run(&["iam", "detach-user-policy", "--user-name", name, "--policy-arn", arn])?;
        }
    }
    for p in aws_json(&["iam", "list-user-policies", "--user-name", name])["PolicyNames"]
        .as_array().into_iter().flatten()
    {
        if let Some(pn) = p.as_str() {
            run(&["iam", "delete-user-policy", "--user-name", name, "--policy-name", pn])?;
        }
    }
    for g in aws_json(&["iam", "list-groups-for-user", "--user-name", name])["Groups"]
        .as_array().into_iter().flatten()
    {
        if let Some(gn) = g["GroupName"].as_str() {
            run(&["iam", "remove-user-from-group", "--group-name", gn, "--user-name", name])?;
        }
    }
    // A login profile / MFA may also exist; ignore failures (none present is the norm).
    let _ = run(&["iam", "delete-login-profile", "--user-name", name]);
    run(&["iam", "delete-user", "--user-name", name])
}

fn delete_iam_policy(arn: &str) -> Result<(), String> {
    let entities = aws_json(&["iam", "list-entities-for-policy", "--policy-arn", arn]);
    for r in entities["PolicyRoles"].as_array().into_iter().flatten() {
        if let Some(n) = r["RoleName"].as_str() {
            run(&["iam", "detach-role-policy", "--role-name", n, "--policy-arn", arn])?;
        }
    }
    for u in entities["PolicyUsers"].as_array().into_iter().flatten() {
        if let Some(n) = u["UserName"].as_str() {
            run(&["iam", "detach-user-policy", "--user-name", n, "--policy-arn", arn])?;
        }
    }
    for g in entities["PolicyGroups"].as_array().into_iter().flatten() {
        if let Some(n) = g["GroupName"].as_str() {
            run(&["iam", "detach-group-policy", "--group-name", n, "--policy-arn", arn])?;
        }
    }
    for v in aws_json(&["iam", "list-policy-versions", "--policy-arn", arn])["Versions"]
        .as_array().into_iter().flatten()
    {
        if v["IsDefaultVersion"].as_bool() == Some(false) {
            if let Some(vid) = v["VersionId"].as_str() {
                run(&["iam", "delete-policy-version", "--policy-arn", arn, "--version-id", vid])?;
            }
        }
    }
    run(&["iam", "delete-policy", "--policy-arn", arn])
}

// ---- aws CLI plumbing ----

fn run(args: &[&str]) -> Result<(), String> {
    let out = Command::new("aws")
        .args(args)
        .output()
        .map_err(|e| format!("could not run `aws` (is the CLI installed + on PATH?): {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Run an `aws ... --output json` read command and parse it; Null on any failure (the
/// caller iterates the missing array as empty).
fn aws_json(args: &[&str]) -> serde_json::Value {
    let Ok(out) = Command::new("aws").args(args).arg("--output").arg("json").output() else {
        return serde_json::Value::Null;
    };
    if !out.status.success() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    matches!(s.trim(), "y" | "Y" | "yes")
}

/// Log-group ARN → log-group name (between ":log-group:" and a trailing ":*").
fn log_group_name(arn: &str) -> String {
    arn.split(":log-group:").nth(1).map(|s| s.trim_end_matches(":*")).unwrap_or(arn).to_string()
}

// ---- inventory scan (mirrors the tag tool) ----

async fn load_marked(
    ddb: &aws_sdk_dynamodb::Client,
    table: &str,
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let mut out = HashSet::new();
    let mut start: Option<std::collections::HashMap<String, AttributeValue>> = None;
    loop {
        let mut req = ddb.scan().table_name(table);
        if let Some(k) = start.take() {
            req = req.set_exclusive_start_key(Some(k));
        }
        let resp = req.send().await?;
        for item in resp.items() {
            let marked = item.get("mark").and_then(|v| v.as_s().ok()).is_some_and(|m| m == "marked");
            if marked {
                if let Some(arn) = item.get("arn").and_then(|v| v.as_s().ok()) {
                    out.insert(arn.clone());
                }
            }
        }
        match resp.last_evaluated_key() {
            Some(k) if !k.is_empty() => start = Some(k.clone()),
            _ => break,
        }
    }
    Ok(out)
}

async fn clear_mark(ddb: &aws_sdk_dynamodb::Client, table: &str, arn: &str) -> Result<(), Box<dyn std::error::Error>> {
    ddb.update_item()
        .table_name(table)
        .key("arn", AttributeValue::S(arn.to_string()))
        .update_expression("REMOVE #m")
        .expression_attribute_names("#m", "mark")
        .send()
        .await?;
    Ok(())
}

fn row_from(r: &Resource, reg: &Registry) -> Row {
    let arn = r.arn().unwrap_or_default().to_string();
    let rtype = r.resource_type().unwrap_or_default().to_string();
    let region = r.region().unwrap_or_default().to_string();
    let service = r.service().unwrap_or_default().to_string();
    let name = manifest_api::classify::display_name(&arn, &rtype);
    let tags = tags_of(r);
    let stack = tags.get("aws:cloudformation:stack-name").cloned();
    let c = classify(&name, &rtype, &service, stack.as_deref(), reg);
    Row { arn, rtype, region, name, protected: c.protected, stack }
}

fn tags_of(r: &Resource) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(prop) = r.properties().iter().find(|p| p.name() == Some("tags")) else {
        return out;
    };
    let Some(data) = prop.data() else {
        return out;
    };
    match data {
        Document::Array(arr) => {
            for item in arr {
                if let Document::Object(map) = item {
                    if let (Some(k), Some(v)) = (map.get("Key").and_then(doc_str), map.get("Value").and_then(doc_str)) {
                        out.insert(k, v);
                    }
                }
            }
        }
        Document::Object(map) => {
            for (k, v) in map {
                if let Some(v) = doc_str(v) {
                    out.insert(k.clone(), v);
                }
            }
        }
        _ => {}
    }
    out
}

fn doc_str(d: &Document) -> Option<String> {
    match d {
        Document::String(s) => Some(s.clone()),
        _ => None,
    }
}
