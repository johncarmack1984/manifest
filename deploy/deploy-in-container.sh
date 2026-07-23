#!/usr/bin/env bash
# Runs INSIDE the deploy toolchain container (see deploy/Dockerfile). Verifies AWS
# credentials, bootstraps CDK once, then builds + deploys manifest. Invoked by
# deploy/bootstrap.sh and by the Codespaces devcontainer; not meant to be run on a
# bare host (it assumes the pinned toolchain is present).
set -euo pipefail

skip_bootstrap=0
for a in "$@"; do
  [ "$a" = "--skip-bootstrap" ] && skip_bootstrap=1
done

if ! aws sts get-caller-identity >/dev/null 2>&1; then
  echo "error: AWS credentials aren't working in the container." >&2
  echo "Provide a profile (AWS_PROFILE, with ~/.aws mounted) or keys" >&2
  echo "(AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN). See deploy/README.md." >&2
  exit 1
fi

account="$(aws sts get-caller-identity --query Account --output text)"
echo "==> deploying manifest into AWS account ${account}"

# Workspace JS deps (also required for the cdk bootstrap + deploy below).
just install

if [ "$skip_bootstrap" -eq 0 ]; then
  # Idempotent: a no-op if the account/region is already bootstrapped. The primary
  # stack is us-east-1; extra MANIFEST_INDEXED_REGIONS need their own bootstrap —
  # re-run with those envs, or bootstrap them by hand (see deploy/README.md).
  echo "==> cdk bootstrap aws://${account}/us-east-1 (idempotent)"
  (cd infra && pnpm exec cdk bootstrap "aws://${account}/us-east-1")
fi

# Build the arm64 Lambdas + the SPA, then deploy every stack.
just api
just web
just up

echo "==> done. The dashboard URL is the stack's 'Url' output above."
