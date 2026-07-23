# manifest deploy toolchain: everything needed to build the Rust Lambdas + the
# React SPA and run `cdk deploy`, pinned. With this image a fresh account needs
# only Docker + AWS credentials locally — no Rust, cargo-lambda, Zig, Node, pnpm,
# just, or CDK on the host. Used by deploy/bootstrap.sh (local one-command deploy)
# and by .devcontainer (GitHub Codespaces / VS Code Dev Containers).
FROM rust:1-slim-bookworm

ENV DEBIAN_FRONTEND=noninteractive \
    PATH=/usr/local/bin:$PATH

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        ca-certificates curl git unzip xz-utils python3 python3-pip; \
    \
    # Node LTS + pnpm (via corepack, matching the repo's packageManager pin).
    curl -fsSL https://deb.nodesource.com/setup_lts.x | bash -; \
    apt-get install -y --no-install-recommends nodejs; \
    corepack enable; \
    corepack prepare pnpm@10.30.3 --activate; \
    \
    # just task runner (the repo's entrypoint: `just deploy`).
    curl -fsSL https://just.systems/install.sh | bash -s -- --to /usr/local/bin; \
    \
    # AWS CLI v2 (bootstrap checks + operator recipes like `just alert-test`).
    arch="$(uname -m)"; \
    curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-${arch}.zip" -o /tmp/awscliv2.zip; \
    unzip -q /tmp/awscliv2.zip -d /tmp; /tmp/aws/install; rm -rf /tmp/aws /tmp/awscliv2.zip; \
    \
    # cargo-lambda + Zig (as a pip package, shimmed onto PATH) for the arm64
    # Lambda cross-compile — same versions the CI deploy uses.
    pip3 install --break-system-packages --no-cache-dir cargo-lambda==1.9.1 ziglang==0.16.0; \
    printf '#!/bin/sh\nexec python3 -m ziglang "$@"\n' > /usr/local/bin/zig; \
    chmod +x /usr/local/bin/zig; \
    rustup target add aarch64-unknown-linux-gnu; \
    \
    apt-get clean; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /work
