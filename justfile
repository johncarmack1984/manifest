# manifest — build + deploy. `just deploy` does everything.
set shell := ["bash", "-uc"]

# Install JS deps for the whole pnpm workspace.
install:
    pnpm install

# Build the Rust Lambda (arm64) that CDK packages.
api:
    cd api && cargo lambda build --release --arm64

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
