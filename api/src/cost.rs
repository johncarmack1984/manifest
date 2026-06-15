use std::collections::{BTreeMap, HashMap};

use aws_sdk_costexplorer::types::{
    DateInterval, Granularity, GroupDefinition, GroupDefinitionType, Metric, MetricValue,
    ResultByTime,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::{AppState, Refresh, Res};

pub async fn handler(
    State(s): State<AppState>,
    _u: AuthUser,
    Query(q): Query<Refresh>,
) -> Result<Json<Value>, StatusCode> {
    if !q.requested() {
        if let Some(v) = crate::cache_get(&s, "cost:v1").await {
            return Ok(Json(v));
        }
    }
    match compute(&s).await {
        Ok(v) => {
            crate::cache_put(&s, "cost:v1", &v).await;
            Ok(Json(v))
        }
        Err(e) => {
            tracing::error!("cost compute failed: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

fn amount(metrics: Option<&HashMap<String, MetricValue>>) -> f64 {
    metrics
        .and_then(|m| m.get("UnblendedCost"))
        .and_then(|v| v.amount())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0)
}

fn flatten_grouped(results: &[ResultByTime]) -> Value {
    let periods: Vec<Value> = results
        .iter()
        .map(|r| {
            let period = r.time_period().map(|t| t.start().to_string()).unwrap_or_default();
            let mut total = 0.0;
            let groups: Vec<Value> = r
                .groups()
                .iter()
                .map(|g| {
                    let key = g.keys().first().cloned().unwrap_or_default();
                    let amt = amount(g.metrics());
                    total += amt;
                    json!({ "key": key, "amount": amt })
                })
                .collect();
            json!({ "period": period, "total": total, "groups": groups })
        })
        .collect();
    json!(periods)
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

/// From daily-by-usage-type data, derive the daily-total series (for the chart),
/// a spike-robust run-rate (median daily × 30), and a one-off breakdown
/// (per usage-type: this-month cost above its own median baseline).
fn analyze(days: &[ResultByTime], today: NaiveDate) -> (Value, Value) {
    let n = days.len().max(1);
    let cur_month = today.format("%Y-%m").to_string();
    let elapsed = today.day() as f64;

    let mut daily_total = vec![0.0_f64; n];
    let mut daily_points: Vec<Value> = Vec::with_capacity(n);
    let mut series: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut mtd: BTreeMap<String, f64> = BTreeMap::new();

    for (i, day) in days.iter().enumerate() {
        let date = day.time_period().map(|t| t.start().to_string()).unwrap_or_default();
        let in_month = date.starts_with(&cur_month);
        let mut day_sum = 0.0;
        for g in day.groups() {
            let ut = g.keys().first().cloned().unwrap_or_default();
            let amt = amount(g.metrics());
            day_sum += amt;
            series.entry(ut.clone()).or_insert_with(|| vec![0.0; n])[i] += amt;
            if in_month {
                *mtd.entry(ut).or_default() += amt;
            }
        }
        daily_total[i] = day_sum;
        daily_points.push(json!({ "date": date, "amount": day_sum }));
    }

    let run_rate = median(&daily_total) * 30.0;

    let mut one_offs: Vec<(String, f64)> = Vec::new();
    for (ut, vals) in &series {
        let baseline = (median(vals) * elapsed).min(*mtd.get(ut).unwrap_or(&0.0));
        let excess = mtd.get(ut).unwrap_or(&0.0) - baseline;
        if excess > 0.50 {
            one_offs.push((ut.clone(), excess));
        }
    }
    one_offs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let one_off_total: f64 = one_offs.iter().map(|(_, a)| a).sum();
    let items: Vec<Value> = one_offs
        .iter()
        .take(8)
        .map(|(u, a)| json!({ "usageType": u, "amount": a }))
        .collect();

    let analysis = json!({
        "runRateMonthly": run_rate,
        "oneOffMtd": one_off_total,
        "oneOffItems": items,
    });
    (json!(daily_points), analysis)
}

async fn compute(s: &AppState) -> Res<Value> {
    let today = Utc::now().date_naive();
    let start_months = (today - Duration::days(95)).format("%Y-%m-%d").to_string();
    let start_30 = (today - Duration::days(30)).format("%Y-%m-%d").to_string();
    let end = today.format("%Y-%m-%d").to_string();
    let monthly = DateInterval::builder().start(&start_months).end(&end).build()?;

    let by_service = s
        .0
        .ce
        .get_cost_and_usage()
        .time_period(monthly.clone())
        .granularity(Granularity::Monthly)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Dimension)
                .key("SERVICE")
                .build(),
        )
        .send()
        .await?;

    let by_region = s
        .0
        .ce
        .get_cost_and_usage()
        .time_period(monthly)
        .granularity(Granularity::Monthly)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Dimension)
                .key("REGION")
                .build(),
        )
        .send()
        .await?;

    // Daily by usage-type powers the daily chart + run-rate + one-off detection.
    let daily_ut = s
        .0
        .ce
        .get_cost_and_usage()
        .time_period(DateInterval::builder().start(&start_30).end(&end).build()?)
        .granularity(Granularity::Daily)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Dimension)
                .key("USAGE_TYPE")
                .build(),
        )
        .send()
        .await?;
    let (daily, run_rate) = analyze(daily_ut.results_by_time(), today);

    // CE's own forecast — kept for comparison (this is the spike-inflated one).
    let fc_end = (today + Duration::days(31)).format("%Y-%m-%d").to_string();
    let forecast = match s
        .0
        .ce
        .get_cost_forecast()
        .time_period(DateInterval::builder().start(&end).end(&fc_end).build()?)
        .metric(Metric::UnblendedCost)
        .granularity(Granularity::Monthly)
        .send()
        .await
    {
        Ok(f) => f.total().and_then(|t| t.amount()).and_then(|a| a.parse::<f64>().ok()),
        Err(e) => {
            tracing::warn!("forecast unavailable: {e}");
            None
        }
    };

    let region_periods = flatten_grouped(by_region.results_by_time());
    let mut billed = std::collections::BTreeSet::new();
    if let Some(arr) = region_periods.as_array() {
        for p in arr {
            if let Some(gs) = p["groups"].as_array() {
                for g in gs {
                    if g["amount"].as_f64().unwrap_or(0.0) > 1.0 {
                        if let Some(k) = g["key"].as_str() {
                            billed.insert(k.to_string());
                        }
                    }
                }
            }
        }
    }
    let uncovered: Vec<String> = billed
        .into_iter()
        .filter(|r| r != "NoRegion" && r != "global" && !s.0.cfg.indexed_regions.contains(r))
        .collect();

    Ok(json!({
        "byService": flatten_grouped(by_service.results_by_time()),
        "byRegion": region_periods,
        "daily": daily,
        "forecastNextMonth": forecast,
        "runRate": run_rate,
        "flags": { "uncoveredRegionsWithSpend": uncovered },
        "generatedAt": Utc::now().to_rfc3339(),
    }))
}
