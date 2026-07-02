# manifest

A self-hosted AWS cost + inventory dashboard. Cost Explorer shows *spend by
service* but never *which resource is the spend* or *what's just sitting there
idle*; manifest does, turning a two-day cleanup into a twenty-minute one.

Deploy it into any AWS account with `just deploy`. Effectively free to run (see
[Cost](#cost)).

## What it shows
- **Cost**: spend by account/app (org-wide via consolidated billing), service,
  region, and day; month-over-month plus a forecast (Cost Explorer, cached in
  DynamoDB ~1h since CE charges $0.01/call).
- **Inventory**: every resource in every indexed region (plus global services: CloudFront, Route53, IAM) in one filterable list (Resource Explorer). Spans org member accounts too: cost is org-wide via consolidated billing, but inventory is per-account, so the Lambda assumes a read-only role in each member account (deploy it with `just member-deploy`; see [Cross-account inventory](#cross-account-inventory)).
- **Cruft flags**: untagged resources, resources in regions with no recent
  deploys, and spend in a region with no inventory coverage (the blind-spot
  detector).

## Architecture
```
<domain> → CloudFront ┬ default → S3 (React SPA, private via OAC)
                       └ /api/*  → Lambda Function URL (Axum, arm64)
Cognito Hosted UI → JWT → Axum validates each request
Axum → Cost Explorer + Resource Explorer + DynamoDB cache
```
Infra is AWS CDK (TypeScript); the API is Rust / Axum on Lambda; the UI
is Vite + React + TypeScript. The SPA fetches all its config from
`/api/config` at runtime, so nothing account-specific is baked into the build.

## Layout
| Dir | What |
|-----|------|
| `infra/` | CDK app: `ManifestStack` (us-east-1) + one `RegionIndexStack` per extra indexed region |
| `api/`   | Rust / Axum Lambda (`provided.al2023`, arm64) |
| `web/`   | Vite + React + TypeScript SPA |

## Prerequisites
- An AWS account + credentials (`aws sso login` / `AWS_PROFILE=…`), admin-ish for the first deploy.
- To see org-wide per-account spend, deploy into your organization's management (payer) account, or a delegated Cost Explorer admin account; from a standalone account it shows only that account.
- A Route53 hosted zone you control (for the dashboard's domain + TLS cert).
- [Rust](https://rustup.rs), [cargo-lambda](https://www.cargo-lambda.info), and [Zig](https://ziglang.org) (to cross-compile the arm64 Lambda).
- Node 20+, [pnpm](https://pnpm.io), and [just](https://github.com/casey/just).
- CDK bootstrapped once per account/region: `pnpm --dir infra exec cdk bootstrap`.

## Configure
```sh
cp infra/.env.example infra/.env
# edit infra/.env: domain, hosted zone id/name, Cognito prefix, owner email
```
`infra/.env` is gitignored; your values never get committed.

## Deploy
```sh
just deploy        # builds the Rust Lambda + the SPA, then `cdk deploy --all`
```
First login: the Cognito user (`MANIFEST_OWNER_EMAIL`) is created with a
temporary password emailed by Cognito; set a real one on first sign-in.

Individual steps: `just api`, `just web`, `just up`, `just synth`, `just destroy`.

## Continuous deployment
Once set up, every push to `main` (a merged PR) deploys itself via GitHub Actions
([`.github/workflows/deploy.yml`](.github/workflows/deploy.yml)): no local `just deploy`.
Auth is GitHub OIDC, so the repo stores no long-lived AWS keys.

One-time setup:
1. Configure `infra/.env` and `cdk bootstrap`, as for a manual deploy.
2. Create the deploy role (once, with admin credentials):
   ```sh
   MANIFEST_GITHUB_REPO=<owner>/<repo> just ci-role
   ```
   It prints `CiDeployRoleArn`. Already have a GitHub OIDC provider in the account?
   Pass `MANIFEST_GITHUB_OIDC_ARN=<arn>` to reuse it (only one is allowed per account).
3. In the repo's **Settings → Secrets and variables → Actions → Variables**, add
   `AWS_DEPLOY_ROLE_ARN` (the printed ARN) and your `MANIFEST_*` config
   (`MANIFEST_DOMAIN_NAME`, `MANIFEST_HOSTED_ZONE_ID`, `MANIFEST_HOSTED_ZONE_NAME`,
   `MANIFEST_COGNITO_DOMAIN_PREFIX`, `MANIFEST_OWNER_EMAIL`, plus any optionals).

These are repo **Variables**, not Secrets: none are sensitive. Until `AWS_DEPLOY_ROLE_ARN`
is set the deploy job is skipped, so `main` stays green on a fresh fork. The deploy role
can only assume the account's CDK bootstrap roles (to run `cdk deploy`) and nothing else.

## Authentication
Out of the box, sign-in uses a Cognito Hosted UI with one admin-created user
(`MANIFEST_OWNER_EMAIL`); Cognito emails a temporary password on first deploy.

To sign in with AWS IAM Identity Center (your AWS SSO) instead, no separate
password, federate Cognito to Identity Center via SAML:

1. Deploy once *without* `MANIFEST_SAML_METADATA_URL`. Note the stack outputs
   `SamlAcsUrl` and `SamlSpEntityId`.
2. In IAM Identity Center → Applications → **Add customer managed application**
   (SAML 2.0): set **Application ACS URL** = `SamlAcsUrl`, **Application SAML
   audience** = `SamlSpEntityId`, add an attribute mapping `email` → `${user:email}`
   (Subject/NameID = email), and assign yourself. Copy the app's **IAM Identity
   Center SAML metadata** URL.
3. Set `MANIFEST_SAML_METADATA_URL=…` in `infra/.env` and `just up` again. Sign-in
   now goes straight to your AWS access portal; the Cognito-local user is removed.

Cognito stays as the OIDC broker (the API validates its JWT); you just never see
a Cognito password again.

## Indexing more regions
Set `MANIFEST_INDEXED_REGIONS=us-east-1,us-west-2,eu-west-1` in `infra/.env`.
Each extra region gets its own tiny `RegionIndexStack`. Cost views always cover
*all* regions regardless; spend in a region you haven't indexed is flagged as a
blind spot.

## Project registry
manifest infers most attribution (AWS-managed services, tooling, CloudFormation
stack names) on its own. The bit it *can't* infer (which resources are dead vs.
protected, and name→repo aliases) lives in a small `projects.toml`. The repo ships
[`api/projects.example.toml`](api/projects.example.toml) (the embedded default); copy
it to `api/projects.toml` (gitignored), edit for your account, and load it live:
```sh
just registry-push   # writes it to the DynamoDB cache; hit Refresh, no redeploy
```
The dashboard reads the registry from DynamoDB and falls back to the embedded default,
so attribution is editable without a deploy. You can also **add an app** straight from
the Inventory view (name + optional match patterns + protected). It's written to the
live registry immediately and the app shows in the picker even before any resource is
assigned. Run `just registry-pull` to sync those live edits back to `projects.toml` for
git (the inverse of `registry-push`).

## Cross-account inventory
Cost is org-wide automatically (consolidated billing covers every linked account), but inventory is per-account: Resource Explorer only sees the account it's queried in. So to inventory other org accounts, manifest assumes a read-only role in each one. The Lambda enumerates the org's accounts (`organizations:ListAccounts`) and, for each, assumes `MANIFEST_MEMBER_ROLE` (default `ManifestInventoryRole`) to sweep that account's regions; resources are tagged with their owning account and filterable in the UI.

Deploy the role into each member account (once per account, with that account's credentials):
```sh
AWS_PROFILE=<member-profile> MANIFEST_PAYER_ACCOUNT=<payer-account-id> just member-deploy
```
The role grants only read-only Resource Explorer + `acm:DescribeCertificate`, and trusts the payer account (the actual caller is gated by the Lambda's narrow `sts:AssumeRole` grant). Each member account must have Resource Explorer enabled (an index + default view) in the regions you want covered. Accounts that can't be reached (role not yet deployed, or no Resource Explorer) show as a "not inventoried" banner rather than silently vanishing. Set `MANIFEST_MEMBER_ROLE=""` to disable and inventory only the dashboard's own account.

## Reaping marked resources
**Mark for deletion** in the Inventory view only records intent in DynamoDB. It deletes
nothing. To actually delete the marked resources, run the operator tool with your own
admin credentials:
```sh
just reap            # dry run: what would be deleted, refused, or skipped
just reap --apply    # delete, confirming each one (--yes to skip the confirms)
```
The dashboard Lambda has **no** delete permissions; `reap` runs locally as you, so
destruction is always a deliberate, reviewable, local step. It **refuses** to delete
CloudFormation/CDK stack members (destroy the stack via its IaC instead) and anything
classified `protected`, and only deletes a curated set of standalone types (IAM
roles/users/policies, Lambda functions, log groups, DynamoDB tables, SNS topics, SES
identities, CloudWatch alarms, S3 buckets), reporting everything else rather than
touching it. A successful delete clears the resource's mark. Needs the AWS CLI on PATH;
set `MANIFEST_RESOURCE_EXPLORER_VIEW_ARN` (the `ResourceExplorerViewArn` stack output) in
`infra/.env` so the scan covers every region.

## Cost
Effectively free: Lambda + DynamoDB on-demand + Resource Explorer (free) +
CloudFront/S3 pennies. The only metered call is Cost Explorer ($0.01/request),
capped by the DynamoDB cache.

## Teardown
```sh
just destroy
```

## License
MIT. See [LICENSE](LICENSE).
