//! Scheduled spend-anomaly alerter. An EventBridge rule invokes this once a day;
//! it pulls the trailing daily cost-per-service and cost-per-account from Cost
//! Explorer, runs manifest's own detector (`manifest_api::anomaly`), and emails a
//! single digest of anything that jumped — via SNS. Deliberately its own thing,
//! not a read of AWS Cost Anomaly Detection: a spend watchdog that doesn't lean
//! on the same console it's meant to backstop.
//!
//! It never emails on a quiet day, and de-dupes per evaluated date (via a marker
//! in the cache table) so a retry or a manual re-run won't double-send.
//!
//! Invoke payload (all optional): `{"force": true}` ignores the de-dupe marker;
//! `{"dry_run": true}` computes and logs the digest but publishes nothing. `just
//! alert-test` invokes the deployed function with both set.
//!
//! Env: CACHE_TABLE, ALERT_TOPIC_ARN, APP_URL, ANOMALY_MIN_DOLLARS (default 5),
//! ANOMALY_PCT (default 50), ANOMALY_BASELINE_DAYS (default 14).

use std::collections::{BTreeMap, HashMap};

use aws_config::BehaviorVersion;
use aws_sdk_costexplorer::types::{
    DateInterval, Granularity, GroupDefinition, GroupDefinitionType, MetricValue, ResultByTime,
};
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{Duration, Utc};
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use manifest_api::anomaly::{detect, render_digest, Section, Thresholds};
use serde_json::{json, Value};

/// A de-dupe marker lives this long — enough that a retry storm or a manual
/// re-run the same day is suppressed, short enough to self-clean via table TTL.
const MARKER_TTL_SECONDS: i64 = 3 * 24 * 3600;

struct Cfg {
    cache_table: String,
    topic_arn: String,
    app_url: String,
    thresholds: Thresholds,
    baseline_days: i64,
}

impl Cfg {
    fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let num = |k: &str, d: f64| var(k).parse().unwrap_or(d);
        Cfg {
            cache_table: var("CACHE_TABLE"),
            topic_arn: var("ALERT_TOPIC_ARN"),
            app_url: var("APP_URL"),
            thresholds: Thresholds {
                min_dollars: num("ANOMALY_MIN_DOLLARS", 5.0),
                pct: num("ANOMALY_PCT", 50.0),
            },
            baseline_days: num("ANOMALY_BASELINE_DAYS", 14.0) as i64,
        }
    }
}

struct Ctx {
    ce: aws_sdk_costexplorer::Client,
    ddb: aws_sdk_dynamodb::Client,
    sns: aws_sdk_sns::Client,
    org: aws_sdk_organizations::Client,
    cfg: Cfg,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .without_time()
        .init();

    let shared = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let ctx = std::sync::Arc::new(Ctx {
        ce: aws_sdk_costexplorer::Client::new(&shared),
        ddb: aws_sdk_dynamodb::Client::new(&shared),
        sns: aws_sdk_sns::Client::new(&shared),
        org: aws_sdk_organizations::Client::new(&shared),
        cfg: Cfg::from_env(),
    });

    run(service_fn(move |e: LambdaEvent<Value>| {
        let ctx = ctx.clone();
        async move { scan(&ctx, e.payload).await }
    }))
    .await
}

/// One alerting pass. Returns a small JSON summary (handy for `just alert-test`).
async fn scan(ctx: &Ctx, payload: Value) -> Result<Value, Error> {
    let force = payload.get("force").and_then(Value::as_bool).unwrap_or(false);
    // With no SNS topic wired there's nothing to send to, so we can only ever
    // compute-and-log — treat that as a dry run.
    let dry_run = payload.get("dry_run").and_then(Value::as_bool).unwrap_or(false)
        || ctx.cfg.topic_arn.is_empty();

    let today = Utc::now().date_naive();
    // CE's "today" is partial and unreliable, so the most recent *complete* day —
    // yesterday — is the one we evaluate. The baseline is the days before it.
    let evaluated = today - Duration::days(1);
    let evaluated_str = evaluated.format("%Y-%m-%d").to_string();
    let start = (today - Duration::days(ctx.cfg.baseline_days + 1)).format("%Y-%m-%d").to_string();
    let end = today.format("%Y-%m-%d").to_string();

    let marker = format!("alert:sent:{evaluated_str}");
    if !force && already_alerted(ctx, &marker).await {
        tracing::info!("already alerted for {evaluated_str}; skipping");
        return Ok(json!({ "status": "already-alerted", "date": evaluated_str }));
    }

    let by_service = daily_series(ctx, "SERVICE", &start, &end).await?;
    let mut by_account = daily_series(ctx, "LINKED_ACCOUNT", &start, &end).await?;
    relabel_accounts(&mut by_account, &account_names(ctx).await);

    let service_hits = detect(&by_service, &ctx.cfg.thresholds);
    let account_hits = detect(&by_account, &ctx.cfg.thresholds);

    let sections = [
        Section { label: "By service", anomalies: &service_hits },
        Section { label: "By account", anomalies: &account_hits },
    ];
    let Some((subject, body)) = render_digest(&evaluated_str, &ctx.cfg.app_url, &sections) else {
        tracing::info!("no anomalies for {evaluated_str}");
        return Ok(json!({ "status": "no-anomalies", "date": evaluated_str }));
    };

    let count = service_hits.len() + account_hits.len();
    if dry_run {
        tracing::info!("[dry-run] would send:\nSubject: {subject}\n{body}");
        return Ok(json!({ "status": "dry-run", "date": evaluated_str, "count": count }));
    }

    ctx.sns
        .publish()
        .topic_arn(&ctx.cfg.topic_arn)
        .subject(&subject)
        .message(&body)
        .send()
        .await?;
    mark_alerted(ctx, &marker, count).await;
    tracing::info!("alerted {count} anomalies for {evaluated_str}");
    Ok(json!({ "status": "sent", "date": evaluated_str, "count": count }))
}

fn amount(metrics: Option<&HashMap<String, MetricValue>>) -> f64 {
    metrics
        .and_then(|m| m.get("UnblendedCost"))
        .and_then(|v| v.amount())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0)
}

/// Cost Explorer daily-by-dimension → the detector's aligned per-key series (each
/// vector length `n`, index `i` = the i-th day, missing keys zero-filled).
async fn daily_series(
    ctx: &Ctx,
    dimension: &str,
    start: &str,
    end: &str,
) -> Result<BTreeMap<String, Vec<f64>>, Error> {
    let out = ctx
        .ce
        .get_cost_and_usage()
        .time_period(DateInterval::builder().start(start).end(end).build()?)
        .granularity(Granularity::Daily)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Dimension)
                .key(dimension)
                .build(),
        )
        .send()
        .await?;

    let days: &[ResultByTime] = out.results_by_time();
    let n = days.len();
    let mut series: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (i, day) in days.iter().enumerate() {
        for g in day.groups() {
            let key = g.keys().first().cloned().unwrap_or_default();
            series.entry(key).or_insert_with(|| vec![0.0; n])[i] += amount(g.metrics());
        }
    }
    Ok(series)
}

/// Account id → name (from Organizations; works from the payer/delegated-admin
/// account). Empty map elsewhere — account anomalies then read as raw ids.
async fn account_names(ctx: &Ctx) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut pages = ctx.org.list_accounts().into_paginator().send();
    while let Some(page) = pages.next().await {
        match page {
            Ok(out) => {
                for a in out.accounts() {
                    if let (Some(id), Some(name)) = (a.id(), a.name()) {
                        map.insert(id.to_string(), name.to_string());
                    }
                }
            }
            Err(e) => {
                tracing::warn!("organizations:ListAccounts unavailable: {e}");
                break;
            }
        }
    }
    map
}

/// Rename account-id keys to names in place (best-effort — unknown ids stay).
fn relabel_accounts(series: &mut BTreeMap<String, Vec<f64>>, names: &HashMap<String, String>) {
    if names.is_empty() {
        return;
    }
    let renamed: BTreeMap<String, Vec<f64>> = std::mem::take(series)
        .into_iter()
        .map(|(id, vals)| (names.get(&id).cloned().unwrap_or(id), vals))
        .collect();
    *series = renamed;
}

async fn already_alerted(ctx: &Ctx, marker: &str) -> bool {
    let out = ctx
        .ddb
        .get_item()
        .table_name(&ctx.cfg.cache_table)
        .key("cache_key", AttributeValue::S(marker.to_string()))
        .send()
        .await;
    match out {
        Ok(o) => o
            .item
            .and_then(|i| i.get("expires_at").and_then(|v| v.as_n().ok()).cloned())
            .and_then(|n| n.parse::<i64>().ok())
            .is_some_and(|exp| exp >= Utc::now().timestamp()),
        Err(e) => {
            tracing::warn!("de-dupe read failed (will not suppress): {e}");
            false
        }
    }
}

async fn mark_alerted(ctx: &Ctx, marker: &str, count: usize) {
    let exp = Utc::now().timestamp() + MARKER_TTL_SECONDS;
    let res = ctx
        .ddb
        .put_item()
        .table_name(&ctx.cfg.cache_table)
        .item("cache_key", AttributeValue::S(marker.to_string()))
        .item("count", AttributeValue::N(count.to_string()))
        .item("expires_at", AttributeValue::N(exp.to_string()))
        .send()
        .await;
    if let Err(e) = res {
        // Worst case is a duplicate email on a retry — never a missed alert.
        tracing::warn!("de-dupe marker write failed: {e}");
    }
}
