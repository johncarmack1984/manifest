#!/usr/bin/env node
import 'source-map-support/register';
import * as path from 'path';
import * as dotenv from 'dotenv';
import * as cdk from 'aws-cdk-lib';
import { loadConfig, PRIMARY_REGION } from '../lib/config';
import { ManifestStack } from '../lib/manifest-stack';
import { RegionIndexStack } from '../lib/region-index-stack';
import { ManifestMemberStack } from '../lib/member-stack';
import { ManifestCiStack } from '../lib/ci-stack';

// Load infra/.env (gitignored) before reading config, regardless of cwd.
dotenv.config({ path: path.join(__dirname, '..', '.env') });

const app = new cdk.App();
const cfg = loadConfig();

// Whatever account your AWS credentials point at. No account id is baked in.
const account = process.env.CDK_DEFAULT_ACCOUNT;

// Tag every resource in every stack.
cdk.Tags.of(app).add('Project', cfg.name);
cdk.Tags.of(app).add('ManagedBy', 'cdk');

// A Resource Explorer LOCAL index in each indexed region except the primary,
// which gets the AGGREGATOR index inside the main stack. A CDK stack is
// single-region, so every extra region is its own (tiny) stack. Adding a
// region is one entry in MANIFEST_INDEXED_REGIONS — no code change.
// Per-region LOCAL indexes — only when we create our own aggregator. Skipped when
// reusing an existing account aggregator (which already has its region indexes).
if (cfg.createAggregator) {
  for (const region of cfg.indexedRegions.filter((r) => r !== PRIMARY_REGION)) {
    new RegionIndexStack(app, `${stackId(cfg.name)}-Index-${region}`, {
      env: { account, region },
    });
  }
}

new ManifestStack(app, stackId(cfg.name), {
  env: { account, region: PRIMARY_REGION },
  cfg,
});

// The member-account role stack is synthesized ONLY when explicitly targeting a
// member account (`just member-deploy`), so `cdk deploy --all` never creates it in
// the payer account. Deploy it with that member's credentials; `account` is then the
// member account, and MANIFEST_PAYER_ACCOUNT names the payer it must trust.
if (process.env.MANIFEST_MEMBER_DEPLOY === '1') {
  if (!cfg.memberInventoryRole) throw new Error('MANIFEST_MEMBER_ROLE is empty — nothing to deploy');
  if (!cfg.payerAccountId) throw new Error('MANIFEST_PAYER_ACCOUNT is required to deploy the member stack');
  // The role is global IAM, so its region is arbitrary — target one the member
  // account is already CDK-bootstrapped in (MANIFEST_MEMBER_REGION) to avoid a bootstrap.
  new ManifestMemberStack(app, `${stackId(cfg.name)}Member`, {
    env: { account, region: process.env.MANIFEST_MEMBER_REGION || PRIMARY_REGION },
    roleName: cfg.memberInventoryRole,
    payerAccountId: cfg.payerAccountId,
  });
}

// The GitHub Actions CI deploy role (OIDC). Synthesized ONLY when explicitly targeting
// it (`just ci-role`), so `cdk deploy --all` never touches it. Deploy once with admin
// credentials; set the printed ARN as the repo variable AWS_DEPLOY_ROLE_ARN.
if (process.env.MANIFEST_CI_DEPLOY === '1') {
  if (!cfg.githubRepo) throw new Error('MANIFEST_GITHUB_REPO ("owner/repo") is required to deploy the CI role');
  new ManifestCiStack(app, `${stackId(cfg.name)}Ci`, {
    env: { account, region: PRIMARY_REGION },
    name: cfg.name,
    githubRepo: cfg.githubRepo,
    githubOidcArn: cfg.githubOidcArn,
    cdkQualifier: cfg.cdkQualifier,
  });
}

/** CloudFormation-safe PascalCase stack id derived from the resource name. */
function stackId(name: string): string {
  return name
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((s) => s[0].toUpperCase() + s.slice(1))
    .join('');
}
