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
                aws_sdk_dynamodb::types::AttributeValue::S("registry:projects.toml".into()),
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
