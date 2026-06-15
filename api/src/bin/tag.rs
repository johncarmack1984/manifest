//! Local operator tool: classify the account's inventory and (optionally) tag it.
//!
//! Dry-run by default — prints the category summary, the per-app grouping, the
//! orphan candidates, and the exact tag plan. Pass `--apply` to write the tags
//! (`app=<repo>`, `manifest:category=<category>`) where the API allows.
//!
//! Runs with YOUR credentials — deliberately NOT the Lambda role, so the hosted
//! dashboard never holds write/tag permissions. Reads MANIFEST_RESOURCE_EXPLORER_VIEW_ARN
//! and MANIFEST_INDEXED_REGIONS from the environment.

use std::collections::{BTreeMap, HashMap};

use aws_config::BehaviorVersion;
use aws_sdk_resourceexplorer2::types::Resource;
use aws_smithy_types::Document;
use manifest_api::classify::{classify, Category};
use manifest_api::registry::Registry;

struct Row {
    arn: String,
    rtype: String,
    region: String,
    name: String,
    category: Category,
    app: Option<String>,
    protected: bool,
    reason: String,
}

impl Row {
    /// Tags this resource should carry: always a category, plus an app when known.
    fn tags(&self) -> Vec<(String, String)> {
        let mut t = vec![("manifest:category".to_string(), self.category.as_str().to_string())];
        if let Some(a) = &self.app {
            t.push(("app".to_string(), a.clone()));
        }
        t
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let apply = std::env::args().any(|a| a == "--apply");
    let reg = Registry::load();
    let shared = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let re = aws_sdk_resourceexplorer2::Client::new(&shared);

    let view_arn = std::env::var("MANIFEST_RESOURCE_EXPLORER_VIEW_ARN").unwrap_or_default();
    let regions: Vec<String> = std::env::var("MANIFEST_INDEXED_REGIONS")
        .unwrap_or_else(|_| "us-east-1".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Pull every resource via Resource Explorer, one region at a time.
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
                rows.push(row_from(r, &reg));
            }
            next = resp.next_token().map(|t| t.to_string());
            if next.is_none() {
                break;
            }
        }
    }

    report(&rows);

    if apply {
        apply_tags(&shared, &rows).await?;
    } else {
        println!("\nDry run — nothing written. Re-run `just tag --apply` to write these tags.");
    }
    Ok(())
}

fn row_from(r: &Resource, reg: &Registry) -> Row {
    let arn = r.arn().unwrap_or_default().to_string();
    let rtype = r.resource_type().unwrap_or_default().to_string();
    let region = r.region().unwrap_or_default().to_string();
    let service = r.service().unwrap_or_default().to_string();
    let name = arn.rsplit(['/', ':']).next().unwrap_or("").to_string();
    let tags = tags_of(r);
    let stack = tags.get("aws:cloudformation:stack-name").map(String::as_str);
    let c = classify(&name, &rtype, &service, stack, reg);
    Row {
        arn,
        rtype,
        region,
        name,
        category: c.category,
        app: c.app,
        protected: c.protected,
        reason: c.reason,
    }
}

/// Resource Explorer returns tags as the `tags` property: an array of {Key, Value}.
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

fn report(rows: &[Row]) {
    let mut by_cat: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_app: BTreeMap<String, usize> = BTreeMap::new();
    for r in rows {
        *by_cat.entry(r.category.as_str()).or_default() += 1;
        if let Some(a) = &r.app {
            *by_app.entry(a.clone()).or_default() += 1;
        }
    }

    println!("== {} resources ==", rows.len());
    for (c, n) in &by_cat {
        println!("  {c:<12} {n:>4}");
    }

    println!("\n== by app ==");
    for (a, n) in &by_app {
        println!("  {a:<26} {n:>4}");
    }

    let dead: Vec<&Row> = rows.iter().filter(|r| r.category == Category::Orphan).collect();
    println!("\n== CONFIRMED ORPHANS — dead/handed-off ({}) ==", dead.len());
    for r in &dead {
        println!("  [{:<10}] {:<54} {:<30} — {}", r.region, r.name, r.rtype, r.reason);
    }

    let unclaimed: Vec<&Row> = rows.iter().filter(|r| r.category == Category::Unclaimed).collect();
    println!("\n== UNCLAIMED — needs attribution ({}) ==", unclaimed.len());
    for r in &unclaimed {
        println!("  [{:<10}] {:<54} {}", r.region, r.name, r.rtype);
    }

    let protected = rows.iter().filter(|r| r.protected).count();
    println!("\n{protected} resources flagged protected (never orphaned/deletable).");
}

async fn apply_tags(shared: &aws_config::SdkConfig, rows: &[Row]) -> Result<(), Box<dyn std::error::Error>> {
    use aws_sdk_resourcegroupstagging as tagging;

    // Group by (region, identical tag set) — TagResources applies one tag set per call.
    let mut groups: BTreeMap<(String, Vec<(String, String)>), Vec<String>> = BTreeMap::new();
    for r in rows {
        groups.entry((r.region.clone(), r.tags())).or_default().push(r.arn.clone());
    }

    let mut tagged = 0usize;
    let mut failed = 0usize;
    for ((region, tags), arns) in groups {
        let conf = tagging::config::Builder::from(shared)
            .region(tagging::config::Region::new(region.clone()))
            .build();
        let client = tagging::Client::from_conf(conf);
        let tagmap: HashMap<String, String> = tags.into_iter().collect();

        for chunk in arns.chunks(20) {
            match client
                .tag_resources()
                .set_resource_arn_list(Some(chunk.to_vec()))
                .set_tags(Some(tagmap.clone()))
                .send()
                .await
            {
                Ok(out) => {
                    let fails = out.failed_resources_map();
                    let n_fail = fails.map_or(0, |m| m.len());
                    failed += n_fail;
                    tagged += chunk.len() - n_fail;
                    if let Some(m) = fails {
                        for (arn, info) in m {
                            println!("  skip {arn}: {}", info.error_message().unwrap_or("untaggable"));
                        }
                    }
                }
                Err(e) => {
                    failed += chunk.len();
                    println!("  region {region} batch failed: {e}");
                }
            }
        }
    }
    println!("\nApplied tags to {tagged} resources; {failed} skipped (untaggable type or error).");
    Ok(())
}
