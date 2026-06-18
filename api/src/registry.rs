//! The thin project registry (projects.toml) — liveness, protection, and aliases
//! that pure inference can't derive.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Project {
    pub repo: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub dead: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct Registry {
    #[serde(default, rename = "project")]
    pub projects: Vec<Project>,
}

// The embedded default the Lambda ships with. A real deployment overrides it at
// runtime from DynamoDB (see `from_dynamo` / `just registry-push`); copy
// projects.example.toml to projects.toml and push it to attribute your own account.
const REGISTRY_TOML: &str = include_str!("../projects.example.toml");

/// DynamoDB cache_key under which the live (hot-reloadable) registry TOML is stored.
pub const REGISTRY_KEY: &str = "registry:projects.toml";

impl Registry {
    pub fn load() -> Self {
        toml::from_str(REGISTRY_TOML).expect("parse projects.toml")
    }

    /// Hot-reloadable registry: read projects.toml from the DynamoDB config item
    /// if present, else fall back to the embedded default. Lets attribution be
    /// edited without a redeploy — `just registry-push` updates the item.
    pub async fn from_dynamo(ddb: &aws_sdk_dynamodb::Client, table: &str) -> Self {
        let got = ddb
            .get_item()
            .table_name(table)
            .key(
                "cache_key",
                aws_sdk_dynamodb::types::AttributeValue::S(REGISTRY_KEY.into()),
            )
            .send()
            .await;
        if let Ok(out) = got {
            if let Some(body) = out.item().and_then(|i| i.get("body")).and_then(|v| v.as_s().ok()) {
                match toml::from_str(body) {
                    Ok(r) => return r,
                    Err(e) => tracing::warn!("DynamoDB registry parse failed, using embedded: {e}"),
                }
            }
        }
        Self::load()
    }

    /// First project whose `types` contains `rtype` or whose `patterns` appear
    /// (case-insensitive) in `text` (a resource name or a CloudFormation stack name).
    pub fn match_project(&self, text: &str, rtype: &str) -> Option<&Project> {
        let lower = text.to_ascii_lowercase();
        self.projects.iter().find(|p| {
            p.types.iter().any(|t| t == rtype)
                || p.patterns.iter().any(|pat| lower.contains(&pat.to_ascii_lowercase()))
        })
    }
}

/// A new app from the dashboard's "Add app" form.
#[derive(Debug, Deserialize)]
pub struct NewApp {
    pub repo: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub dead: bool,
    #[serde(default)]
    pub reason: String,
}

/// Append an app to the live registry in DynamoDB, preserving the existing comments and
/// formatting (toml_edit). Reads the current live registry (or the embedded seed if
/// nothing's been pushed yet), rejects a duplicate repo, appends the new `[[project]]`,
/// and writes it back; the next inventory load hot-reloads it. `just registry-pull`
/// syncs the result back to projects.toml for git.
pub async fn add_app(ddb: &aws_sdk_dynamodb::Client, table: &str, app: &NewApp) -> Result<(), String> {
    use aws_sdk_dynamodb::types::AttributeValue;
    use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

    let repo = app.repo.trim();
    if repo.is_empty() {
        return Err("app name is required".into());
    }

    let current = ddb
        .get_item()
        .table_name(table)
        .key("cache_key", AttributeValue::S(REGISTRY_KEY.into()))
        .send()
        .await
        .ok()
        .and_then(|o| o.item().and_then(|i| i.get("body")).and_then(|v| v.as_s().ok()).cloned())
        .unwrap_or_else(|| REGISTRY_TOML.to_string());

    let mut doc = current.parse::<DocumentMut>().map_err(|e| format!("registry parse error: {e}"))?;
    let projects = doc
        .entry("project")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| "registry's `project` is not an array of tables".to_string())?;

    if projects.iter().any(|t| t.get("repo").and_then(|v| v.as_str()) == Some(repo)) {
        return Err(format!("app '{repo}' already exists"));
    }

    let mut t = Table::new();
    t.insert("repo", value(repo));
    let patterns: Array = app.patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    if !patterns.is_empty() {
        t.insert("patterns", value(patterns));
    }
    if app.protected {
        t.insert("protected", value(true));
    }
    if app.dead {
        t.insert("dead", value(true));
    }
    let reason = app.reason.trim();
    if !reason.is_empty() {
        t.insert("reason", value(reason));
    }
    projects.push(t);

    ddb.put_item()
        .table_name(table)
        .item("cache_key", AttributeValue::S(REGISTRY_KEY.into()))
        .item("body", AttributeValue::S(doc.to_string()))
        .send()
        .await
        .map_err(|e| format!("registry write failed: {e}"))?;
    Ok(())
}
