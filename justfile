# manifest — build + deploy. `just deploy` does everything.
set shell := ["bash", "-uc"]

# Install JS deps for the whole pnpm workspace.
install:
    pnpm install

# Build the Rust Lambda (arm64) that CDK packages.
api:
    cd api && cargo lambda build --release --arm64 --bin manifest-api

# Classify + tag the account inventory (dry-run by default; pass --apply to write).
tag *args:
    cd api && set -a && . ../infra/.env && set +a && AWS_REGION=us-east-1 cargo run --quiet --bin tag -- {{args}}

# Delete resources you flagged "mark for deletion" in the dashboard. Dry-run by default;
# `just reap --apply` deletes (per-resource confirm), `--yes` skips confirms. Runs with
# YOUR credentials; refuses CFN/CDK stack members + protected resources. See the README.
reap *args:
    cd api && set -a && . ../infra/.env && set +a && AWS_REGION=us-east-1 cargo run --quiet --bin reap -- {{args}}

# Push your local projects.toml to the live registry the dashboard reads (no deploy; hit
# Refresh after). projects.toml is gitignored — copy projects.example.toml and edit it.
registry-push:
    cd api && \
      test -f projects.toml || { echo "no api/projects.toml — copy projects.example.toml, edit it, then re-run"; exit 1; } && \
      body=$(jq -Rs . projects.toml) && \
      aws dynamodb put-item --region us-east-1 --table-name manifest-cache \
        --item "{\"cache_key\":{\"S\":\"registry:projects.toml\"},\"body\":{\"S\":$body}}" && \
      echo "✓ registry pushed — hit Refresh on the dashboard"

# Pull the LIVE registry (DynamoDB — includes apps added from the dashboard) back to
# projects.toml so you can commit it. The inverse of registry-push.
registry-pull:
    cd api && aws dynamodb get-item --region us-east-1 --table-name manifest-cache \
      --key '{"cache_key":{"S":"registry:projects.toml"}}' --query 'Item.body.S' --output text > projects.toml \
      && echo "✓ pulled live registry → api/projects.toml"

# Build the React SPA.
web:
    cd web && pnpm build

# Deploy all CDK stacks (expects api + web already built).
up:
    cd infra && pnpm exec cdk deploy --all --require-approval never

# Synthesize CloudFormation without deploying (uses placeholders if not built).
synth:
    cd infra && pnpm exec cdk synth

# Deploy the cross-account inventory role INTO an org member account so the dashboard
# can inventory it (cost is already org-wide; inventory is per-account). Run once per
# member account with that account's credentials:
#   AWS_PROFILE=<member> MANIFEST_PAYER_ACCOUNT=<payer-account-id> just member-deploy
# The member account must have Resource Explorer enabled (an index + default view).
member-deploy:
    cd infra && MANIFEST_MEMBER_DEPLOY=1 pnpm exec cdk deploy '*Member' --require-approval never

# Deploy the GitHub Actions CI deploy role (OIDC) into THIS account. Run once with admin
# credentials, then set the printed role ARN as the repo variable AWS_DEPLOY_ROLE_ARN.
# Reuse an existing GitHub OIDC provider with MANIFEST_GITHUB_OIDC_ARN if you have one.
#   MANIFEST_GITHUB_REPO=owner/repo just ci-role
ci-role:
    cd infra && MANIFEST_CI_DEPLOY=1 pnpm exec cdk deploy '*Ci' --require-approval never

# Full build + deploy.
deploy: install api web up

# Tear everything down.
destroy:
    cd infra && pnpm exec cdk destroy --all
