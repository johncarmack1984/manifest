//! Durable per-resource operator state (keyed by ARN), in the `manifest-state`
//! table — currently classification overrides (deletion marks land here next).
//! The dashboard only ever reads and writes this state; it never mutates the AWS
//! resources themselves.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use crate::{AppState, Res};

#[derive(Default, Clone)]
pub struct ResourceState {
    /// Manual classification: attribute this resource to this app, overriding inference.
    pub app: Option<String>,
}

/// Load every resource's operator state, keyed by ARN. The table holds only the
/// resources the operator has actually touched, so this scan stays small.
pub async fn load(s: &AppState) -> HashMap<String, ResourceState> {
    let mut out = HashMap::new();
    if s.0.cfg.state_table.is_empty() {
        return out;
    }
    let mut start: Option<HashMap<String, AttributeValue>> = None;
    loop {
        let mut req = s.0.ddb.scan().table_name(&s.0.cfg.state_table);
        if let Some(k) = start.take() {
            req = req.set_exclusive_start_key(Some(k));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("state scan failed: {e}");
                break;
            }
        };
        for item in resp.items() {
            let Some(arn) = item.get("arn").and_then(|v| v.as_s().ok()) else {
                continue;
            };
            out.insert(arn.clone(), ResourceState { app: str_attr(item, "app") });
        }
        match resp.last_evaluated_key() {
            Some(k) if !k.is_empty() => start = Some(k.clone()),
            _ => break,
        }
    }
    out
}

fn str_attr(item: &HashMap<String, AttributeValue>, key: &str) -> Option<String> {
    item.get(key).and_then(|v| v.as_s().ok()).cloned()
}

/// Set (`Some`) or clear (`None`) a resource's classification override, preserving
/// any other state on the item (e.g. a deletion mark).
pub async fn set_override(s: &AppState, arn: &str, app: Option<&str>) -> Res<()> {
    let req = s
        .0
        .ddb
        .update_item()
        .table_name(&s.0.cfg.state_table)
        .key("arn", AttributeValue::S(arn.to_string()))
        .expression_attribute_names("#app", "app");
    let req = match app {
        Some(app) => req
            .update_expression("SET #app = :a")
            .expression_attribute_values(":a", AttributeValue::S(app.to_string())),
        None => req.update_expression("REMOVE #app"),
    };
    req.send().await?;
    Ok(())
}
