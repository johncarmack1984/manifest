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
    // Derivative launch/registration records — the real resource is the ec2:instance.
    // Both accumulate ghosts in Resource Explorer's index long after deletion (every
    // Auto Scaling / Batch scale-out leaves an instant-fleet record behind).
    "ec2:fleet",
    "ssm:managed-instance",
    // Sub-resources of an HTTP API — attributed/deleted through their parent API.
    "apigateway:apis/stages",
    "apigateway:apis/routes",
    "apigateway:apis/integrations",
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
    "iam:mfa",
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

/// Display (and classification) name for a resource ARN. The generic tail-of-ARN is
/// right for most types, but pathed names would lose their meaningful prefix to it:
/// the secret `lux/apple-signing` would surface (and classify!) as its random-suffixed
/// tail `apple-signing-7zTvyl`, and `/aws/batch/job` as `job`. Keep the path for the
/// types that carry one.
pub fn display_name(arn: &str, rtype: &str) -> String {
    fn tail(a: &str) -> String {
        a.rsplit(['/', ':']).next().unwrap_or("").to_string()
    }
    let name = match rtype {
        // arn:…:secret:PATH/NAME-XXXXXX — keep the path, drop Secrets Manager's
        // random six-character suffix.
        "secretsmanager:secret" => {
            let name = arn.splitn(7, ':').nth(6).unwrap_or_default();
            name.rsplit_once('-').map(|(base, _)| base).unwrap_or(name).to_string()
        }
        // arn:…:log-group:/aws/lambda/foo (sometimes ":*"-suffixed) — the path IS the name.
        "logs:log-group" => {
            arn.splitn(7, ':').nth(6).unwrap_or_default().trim_end_matches(":*").to_string()
        }
        // arn:…:parameter/jobs/origin-secret — keep the full parameter path.
        "ssm:parameter" => {
            arn.split_once(":parameter").map(|(_, p)| p.to_string()).unwrap_or_default()
        }
        // arn:…:job-definition/name:3 — the family name (with revision), not "3".
        "batch:job-definition" => {
            arn.split_once("job-definition/").map(|(_, p)| p.to_string()).unwrap_or_default()
        }
        _ => tail(arn),
    };
    if name.is_empty() {
        tail(arn)
    } else {
        name
    }
}

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
        // IAM service-linked roles (AWSServiceRoleFor…) and Identity Center / SSO
        // roles (AWSReservedSSO_…) are AWS-owned plumbing, surfaced once we scan global.
        || lname.starts_with("awsservicerolefor")
        || lname.starts_with("awsreservedsso_")
        || name == "AwsDataCatalog"
        // Service-owned secrets (events!connection/…, rds!db-…): '!' can't appear in a
        // user-created secret name, and the owning service manages their lifecycle.
        || (rtype == "secretsmanager:secret" && name.contains('!'))
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

#[cfg(test)]
mod tests {
    use super::display_name;

    #[test]
    fn pathed_names_keep_their_prefix() {
        let cases = [
            // Secrets keep the namespace and lose the random suffix.
            (
                "arn:aws:secretsmanager:us-west-1:1:secret:lux/apple-signing-7zTvyl",
                "secretsmanager:secret",
                "lux/apple-signing",
            ),
            // Log groups are their full path.
            ("arn:aws:logs:us-east-2:1:log-group:/aws/batch/job", "logs:log-group", "/aws/batch/job"),
            ("arn:aws:logs:us-east-2:1:log-group:/john-voice/train:*", "logs:log-group", "/john-voice/train"),
            // SSM parameters keep their path.
            ("arn:aws:ssm:us-east-1:1:parameter/jobs/origin-secret", "ssm:parameter", "/jobs/origin-secret"),
            // Batch job definitions are the family:revision, not the revision.
            ("arn:aws:batch:us-east-2:1:job-definition/train:2", "batch:job-definition", "train:2"),
            // Everything else stays the ARN tail.
            ("arn:aws:s3:::my-bucket", "s3:bucket", "my-bucket"),
            ("arn:aws:ec2:us-east-1:1:instance/i-0abc", "ec2:instance", "i-0abc"),
        ];
        for (arn, rtype, want) in cases {
            assert_eq!(display_name(arn, rtype), want, "for {arn}");
        }
    }
}
