//! The thin project registry (projects.toml) — liveness, protection, and aliases
//! that pure inference can't derive.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
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

    /// Like `from_dynamo`, but returns `None` when the live registry item is absent or
    /// unparseable instead of falling back to the embedded example. Tools that ACT on
    /// classification (the reaper) must refuse to run without the real registry —
    /// otherwise their protected-resource guard would check the wrong data.
    pub async fn try_from_dynamo(ddb: &aws_sdk_dynamodb::Client, table: &str) -> Option<Self> {
        let out = ddb
            .get_item()
            .table_name(table)
            .key("cache_key", aws_sdk_dynamodb::types::AttributeValue::S(REGISTRY_KEY.into()))
            .send()
            .await
            .ok()?;
        let body = out.item().and_then(|i| i.get("body")).and_then(|v| v.as_s().ok())?;
        toml::from_str(body).ok()
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

/// An app definition from the dashboard's "Add app" / "Edit app" form.
#[derive(Debug, Deserialize)]
pub struct NewApp {
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

/// The live registry TOML (or the embedded seed if nothing's been pushed yet).
async fn live_toml(ddb: &aws_sdk_dynamodb::Client, table: &str) -> String {
    use aws_sdk_dynamodb::types::AttributeValue;
    ddb.get_item()
        .table_name(table)
        .key("cache_key", AttributeValue::S(REGISTRY_KEY.into()))
        .send()
        .await
        .ok()
        .and_then(|o| o.item().and_then(|i| i.get("body")).and_then(|v| v.as_s().ok()).cloned())
        .unwrap_or_else(|| REGISTRY_TOML.to_string())
}

async fn write_toml(ddb: &aws_sdk_dynamodb::Client, table: &str, body: String) -> Result<(), String> {
    use aws_sdk_dynamodb::types::AttributeValue;
    ddb.put_item()
        .table_name(table)
        .item("cache_key", AttributeValue::S(REGISTRY_KEY.into()))
        .item("body", AttributeValue::S(body))
        .send()
        .await
        .map_err(|e| format!("registry write failed: {e}"))?;
    Ok(())
}

/// Write the form's fields onto a `[[project]]` table. Empty lists/strings and false
/// booleans REMOVE their key, so an edit that clears a field really clears it (and a
/// fresh add stays minimal).
fn apply_fields(t: &mut toml_edit::Table, app: &NewApp) {
    use toml_edit::{value, Array};
    let list = |items: &[String]| -> Array {
        items.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect()
    };
    let set_list = |t: &mut toml_edit::Table, key: &str, items: &[String]| {
        let a = list(items);
        if a.is_empty() {
            t.remove(key);
        } else {
            t.insert(key, value(a));
        }
    };
    let set_flag = |t: &mut toml_edit::Table, key: &str, on: bool| {
        if on {
            t.insert(key, value(true));
        } else {
            t.remove(key);
        }
    };
    set_list(t, "patterns", &app.patterns);
    set_list(t, "types", &app.types);
    set_flag(t, "protected", app.protected);
    set_flag(t, "dead", app.dead);
    let reason = app.reason.trim();
    if reason.is_empty() {
        t.remove("reason");
    } else {
        t.insert("reason", value(reason));
    }
}

/// Append an app to the live registry in DynamoDB, preserving the existing comments and
/// formatting (toml_edit). Reads the current live registry (or the embedded seed if
/// nothing's been pushed yet), rejects a duplicate repo, appends the new `[[project]]`,
/// and writes it back; the next inventory load hot-reloads it. `just registry-pull`
/// syncs the result back to projects.toml for git.
pub async fn add_app(ddb: &aws_sdk_dynamodb::Client, table: &str, app: &NewApp) -> Result<(), String> {
    use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

    let repo = app.repo.trim();
    if repo.is_empty() {
        return Err("app name is required".into());
    }

    let current = live_toml(ddb, table).await;
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
    apply_fields(&mut t, app);
    projects.push(t);

    write_toml(ddb, table, doc.to_string()).await
}

/// Update an existing app's rules in the live registry, in place — order and
/// surrounding comments are preserved (toml_edit), so match precedence ("first match
/// wins") doesn't shift underneath an edit. The app is looked up by `repo` (no rename:
/// per-ARN overrides in the state table refer to apps by name).
pub async fn update_app(ddb: &aws_sdk_dynamodb::Client, table: &str, app: &NewApp) -> Result<(), String> {
    use toml_edit::DocumentMut;

    let repo = app.repo.trim();
    if repo.is_empty() {
        return Err("app name is required".into());
    }

    let current = live_toml(ddb, table).await;
    let mut doc = current.parse::<DocumentMut>().map_err(|e| format!("registry parse error: {e}"))?;
    let projects = doc
        .get_mut("project")
        .and_then(|p| p.as_array_of_tables_mut())
        .ok_or_else(|| "registry has no apps yet".to_string())?;

    let t = projects
        .iter_mut()
        .find(|t| t.get("repo").and_then(|v| v.as_str()) == Some(repo))
        .ok_or_else(|| format!("app '{repo}' not found in the registry"))?;
    apply_fields(t, app);

    write_toml(ddb, table, doc.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::DocumentMut;

    fn edit_example(app: &NewApp) -> String {
        // The doc-edit core of update_app, minus DynamoDB: find by repo, apply, serialize.
        let mut doc = REGISTRY_TOML.parse::<DocumentMut>().unwrap();
        let projects = doc.get_mut("project").and_then(|p| p.as_array_of_tables_mut()).unwrap();
        let t = projects
            .iter_mut()
            .find(|t| t.get("repo").and_then(|v| v.as_str()) == Some(app.repo.as_str()))
            .expect("repo in example registry");
        apply_fields(t, app);
        doc.to_string()
    }

    #[test]
    fn update_rewrites_rules_and_preserves_the_rest() {
        let out = edit_example(&NewApp {
            repo: "example-api".into(),
            patterns: vec!["example-api".into(), " exampleapistack ".into(), "".into()],
            types: vec!["sqs:queue".into()],
            protected: true,
            dead: false,
            reason: String::new(),
        });
        // The edited project re-parses with its new rules…
        let reg: Registry = toml::from_str(&out).unwrap();
        let p = reg.projects.iter().find(|p| p.repo == "example-api").unwrap();
        assert_eq!(p.patterns, vec!["example-api", "exampleapistack"]);
        assert_eq!(p.types, vec!["sqs:queue"]);
        assert!(p.protected && !p.dead);
        // …its position is unchanged (match precedence is order-sensitive)…
        let embedded = Registry::load();
        let before: Vec<&str> = embedded.projects.iter().map(|p| p.repo.as_str()).collect();
        let after: Vec<&str> = reg.projects.iter().map(|p| p.repo.as_str()).collect();
        assert_eq!(before, after);
        // …and the file's comments survive the rewrite.
        assert!(out.contains("First match wins"));
    }

    #[test]
    fn update_clears_emptied_fields() {
        // old-prototype starts dead with a reason; an edit that unchecks dead removes both.
        let out = edit_example(&NewApp {
            repo: "old-prototype".into(),
            patterns: vec!["old-prototype".into()],
            types: vec![],
            protected: false,
            dead: false,
            reason: String::new(),
        });
        let reg: Registry = toml::from_str(&out).unwrap();
        let p = reg.projects.iter().find(|p| p.repo == "old-prototype").unwrap();
        assert!(!p.dead && p.reason.is_empty() && p.types.is_empty());
    }
}
