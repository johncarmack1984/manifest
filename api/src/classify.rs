//! Classify an AWS resource into {aws-managed, tooling, app:<repo>, orphan}.
//! Order matters: managed and tooling are recognized before project matching, and
//! the CloudFormation stack-name tag is tried before the resource's own name (it's
//! higher-signal — it catches ID-named resources like Cognito pools or ACM certs).

use crate::registry::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    AwsManaged,
    Tooling,
    App,
    /// Confirmed dead/handed-off (matched a registry `dead` project).
    Orphan,
    /// Matched nothing — could be a real orphan or an app resource that's just
    /// ID-named/untagged. Needs attribution before it's safe to act on.
    Unclaimed,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::AwsManaged => "aws-managed",
            Category::Tooling => "tooling",
            Category::App => "app",
            Category::Orphan => "orphan",
            Category::Unclaimed => "unclaimed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub category: Category,
    pub app: Option<String>,
    pub protected: bool,
    pub reason: String,
}

/// AWS-managed defaults / plumbing / things that are dangerous or pointless to
/// delete via a dashboard. Filtered from the orphan view; never flagged.
const AWS_MANAGED_TYPES: &[&str] = &[
    "kms:key",
    "ec2:vpc",
    "ec2:security-group",
    "ec2:subnet",
    "ec2:route-table",
    "ec2:network-acl",
    "ec2:internet-gateway",
    "ec2:dhcp-options",
    "ec2:security-group-rule",
    "lambda:layer/version",
    "lambda:function/version",
    "resource-explorer-2:index",
    "cloudformation:stack",
    "rds:og",
    "rds:pg",
    "rds:subgrp",
    "rds:secgrp",
    "athena:workgroup",
    "athena:datacatalog",
    "memorydb:acl",
    "memorydb:user",
    "memorydb:parametergroup",
    "memorydb:subnetgroup",
    "elasticache:user",
    "events:event-bus",
    "xray:sampling-rule",
    "backup:backup-plan",
    "backup:backup-vault",
];

/// Substrings that mark IaC/CLI tooling artifacts — owned by a stack, removed by
/// destroying that stack, not individually.
const TOOLING_MARKERS: &[&str] = &[
    "cdk-hnb659fds",
    "CustomCDKBucketDeployment",
    "CustomS3AutoDeleteObjects",
    "AppManager-CFN-",
    "CDKToolkit",
    "SSTConsole",
    "sst-asset",
    "aws-sam-cli",
];

pub fn classify(
    name: &str,
    rtype: &str,
    service: &str,
    stack: Option<&str>,
    reg: &Registry,
) -> Classification {
    let lname = name.to_ascii_lowercase();

    // 1. AWS-managed defaults / plumbing.
    if AWS_MANAGED_TYPES.contains(&rtype)
        || lname.starts_with("default")
        || name == "AwsDataCatalog"
        || (service == "apprunner" && !name.is_empty() && name.chars().all(|c| c == '0' || c == '1'))
    {
        return Classification {
            category: Category::AwsManaged,
            app: None,
            protected: true,
            reason: format!("AWS-managed ({rtype})"),
        };
    }

    // 2. Tooling artifacts (by name, type, or owning tooling stack).
    if rtype == "resource-explorer-2:view"
        || TOOLING_MARKERS.iter().any(|m| name.contains(m))
        || stack.is_some_and(|s| s.contains("CDKToolkit"))
    {
        return Classification {
            category: Category::Tooling,
            app: Some("tooling".into()),
            protected: true,
            reason: "IaC/CLI tooling artifact".into(),
        };
    }

    // 3. Owning CloudFormation stack, then the resource name, against the registry.
    if let Some(s) = stack {
        if let Some(c) = from_registry(reg, s, rtype) {
            return c;
        }
    }
    if let Some(c) = from_registry(reg, name, rtype) {
        return c;
    }

    // 4. Nothing claims it.
    Classification {
        category: Category::Unclaimed,
        app: None,
        protected: false,
        reason: "matches no known project".into(),
    }
}

fn from_registry(reg: &Registry, text: &str, rtype: &str) -> Option<Classification> {
    let p = reg.match_project(text, rtype)?;
    Some(if p.dead {
        Classification {
            category: Category::Orphan,
            app: Some(p.repo.clone()),
            protected: false,
            reason: format!("dead project '{}': {}", p.repo, p.reason),
        }
    } else {
        Classification {
            category: Category::App,
            app: Some(p.repo.clone()),
            protected: p.protected,
            reason: if p.protected {
                format!("protected ({})", p.repo)
            } else {
                format!("project '{}'", p.repo)
            },
        }
    })
}
