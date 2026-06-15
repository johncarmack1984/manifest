#!/usr/bin/env node
import 'source-map-support/register';
import * as path from 'path';
import * as dotenv from 'dotenv';
import * as cdk from 'aws-cdk-lib';
import { loadConfig, PRIMARY_REGION } from '../lib/config';
import { ManifestStack } from '../lib/manifest-stack';
import { RegionIndexStack } from '../lib/region-index-stack';

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
for (const region of cfg.indexedRegions.filter((r) => r !== PRIMARY_REGION)) {
  new RegionIndexStack(app, `${stackId(cfg.name)}-Index-${region}`, {
    env: { account, region },
  });
}

new ManifestStack(app, stackId(cfg.name), {
  env: { account, region: PRIMARY_REGION },
  cfg,
});

/** CloudFormation-safe PascalCase stack id derived from the resource name. */
function stackId(name: string): string {
  return name
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((s) => s[0].toUpperCase() + s.slice(1))
    .join('');
}
