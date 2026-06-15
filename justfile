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

# Push local projects.toml to the live registry the dashboard reads (no deploy; hit Refresh after).
registry-push:
    cd api && body=$(jq -Rs . projects.toml) && \
      aws dynamodb put-item --region us-east-1 --table-name manifest-cache \
        --item "{\"cache_key\":{\"S\":\"registry:projects.toml\"},\"body\":{\"S\":$body}}" && \
      echo "✓ registry pushed — hit Refresh on the dashboard"

# Build the React SPA.
web:
    cd web && pnpm build

# Deploy all CDK stacks (expects api + web already built).
up:
    cd infra && pnpm exec cdk deploy --all --require-approval never

# Synthesize CloudFormation without deploying (uses placeholders if not built).
synth:
    cd infra && pnpm exec cdk synth

# Full build + deploy.
deploy: install api web up

# Tear everything down.
destroy:
    cd infra && pnpm exec cdk destroy --all
