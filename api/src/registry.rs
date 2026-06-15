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

const REGISTRY_TOML: &str = include_str!("../projects.toml");

impl Registry {
    pub fn load() -> Self {
        toml::from_str(REGISTRY_TOML).expect("parse projects.toml")
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
