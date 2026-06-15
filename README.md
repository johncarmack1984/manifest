# manifest

A self-hosted **AWS cost + inventory dashboard**. Cost Explorer shows *spend by
service* but never *which resource is the spend* or *what's just sitting there
idle* — manifest does, turning a two-day cleanup into a twenty-minute one.

Deploy it into any AWS account with `just deploy`. Effectively free to run (see
[Cost](#cost)).

## What it shows
- **Cost** — spend by service × region × day, month-over-month, and a forecast
  (Cost Explorer, cached in DynamoDB ~1h since CE charges $0.01/call).
- **Inventory** — every resource in every indexed region in one filterable list
  (Resource Explorer).
- **Cruft flags** — untagged resources, resources in regions with no recent
  deploys, and **spend in a region with no inventory coverage** (the blind-spot
  detector).

## Architecture
```
<domain> → CloudFront ┬ default → S3 (React SPA, private via OAC)
                       └ /api/*  → Lambda Function URL (Axum, arm64)
Cognito Hosted UI → JWT → Axum validates each request
Axum → Cost Explorer + Resource Explorer + DynamoDB cache
```
Infra is **AWS CDK (TypeScript)**; the API is **Rust / Axum** on Lambda; the UI
is **Vite + React + TypeScript**. The SPA fetches all its config from
`/api/config` at runtime, so nothing account-specific is baked into the build.

## Layout
| Dir | What |
|-----|------|
| `infra/` | CDK app — `ManifestStack` (us-east-1) + one `RegionIndexStack` per extra indexed region |
| `api/`   | Rust / Axum Lambda (`provided.al2023`, arm64) |
| `web/`   | Vite + React + TypeScript SPA |

## Prerequisites
- An AWS account + credentials (`aws sso login` / `AWS_PROFILE=…`), admin-ish for the first deploy.
- A **Route53 hosted zone** you control (for the dashboard's domain + TLS cert).
- [Rust](https://rustup.rs), [cargo-lambda](https://www.cargo-lambda.info), and [Zig](https://ziglang.org) (to cross-compile the arm64 Lambda).
- Node 20+, [pnpm](https://pnpm.io), and [just](https://github.com/casey/just).
- CDK bootstrapped once per account/region: `pnpm --dir infra exec cdk bootstrap`.

## Configure
```sh
cp infra/.env.example infra/.env
# edit infra/.env: domain, hosted zone id/name, Cognito prefix, owner email
```
`infra/.env` is gitignored — your values never get committed.

## Deploy
```sh
just deploy        # builds the Rust Lambda + the SPA, then `cdk deploy --all`
```
First login: the Cognito user (`MANIFEST_OWNER_EMAIL`) is created with a
temporary password emailed by Cognito; set a real one on first sign-in.

Individual steps: `just api`, `just web`, `just up`, `just synth`, `just destroy`.

## Indexing more regions
Set `MANIFEST_INDEXED_REGIONS=us-east-1,us-west-2,eu-west-1` in `infra/.env`.
Each extra region gets its own tiny `RegionIndexStack`. Cost views always cover
*all* regions regardless; spend in a region you haven't indexed is flagged as a
blind spot.

## Cost
Effectively free: Lambda + DynamoDB on-demand + Resource Explorer (free) +
CloudFront/S3 pennies. The only metered call is Cost Explorer ($0.01/request),
capped by the DynamoDB cache.

## Teardown
```sh
just destroy
```

## License
MIT — see [LICENSE](LICENSE).
