#!/usr/bin/env bash
# manifest one-command deploy. The only things you need on this machine are Docker
# and AWS credentials — no Rust, cargo-lambda, Zig, Node, pnpm, just, or CDK. It
# builds the pinned toolchain image (deploy/Dockerfile) and runs `just deploy`
# inside it against your account.
#
# Usage:
#   deploy/bootstrap.sh                 # build image, cdk bootstrap, deploy
#   deploy/bootstrap.sh --skip-bootstrap   # skip cdk bootstrap (already done)
#
# Prerequisites (see deploy/README.md for detail):
#   - Docker running.
#   - AWS credentials: either a profile (export AWS_PROFILE=…, with ~/.aws present)
#     or exported keys (AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN).
#   - infra/.env filled in (cp infra/.env.example infra/.env, then edit).
#   - A Route53 hosted zone you control (for the dashboard's domain + TLS).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
image="manifest-deploy"

case "${1:-}" in
  -h|--help)
    # Print the leading comment block (skip the shebang; stop at the first code line).
    awk 'NR==1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
    exit 0
    ;;
esac

command -v docker >/dev/null 2>&1 || {
  echo "error: Docker is required but not found on PATH." >&2
  exit 1
}
if [ ! -f "$repo/infra/.env" ]; then
  echo "error: $repo/infra/.env not found." >&2
  echo "Run: cp infra/.env.example infra/.env  — then edit it (domain, hosted zone, email)." >&2
  exit 1
fi

echo "==> building the manifest deploy toolchain image (first run is slow; cached after)"
docker build -t "$image" "$here"

# Mount ~/.aws read-only (for SSO/profile creds) and forward any AWS_* env creds.
aws_mount=()
[ -d "$HOME/.aws" ] && aws_mount=(-v "$HOME/.aws:/root/.aws:ro")

# Only allocate a TTY when we actually have one (so this still works in CI).
tty=()
[ -t 0 ] && tty=(-it)

echo "==> deploying (repo mounted at /work; build outputs land in ./api/target and ./web/dist)"
docker run --rm "${tty[@]}" \
  -v "$repo:/work" -w /work \
  "${aws_mount[@]}" \
  -e AWS_PROFILE -e AWS_REGION -e AWS_DEFAULT_REGION \
  -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY -e AWS_SESSION_TOKEN \
  "$image" bash deploy/deploy-in-container.sh "$@"
