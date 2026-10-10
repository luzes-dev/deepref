ARG NODE_BASE_REPOSITORY="docker.io/library/node"
ARG CADDY_BASE_REPOSITORY="docker.io/library/caddy"

# The web build emits architecture-independent static assets, so run it on the
# native BuildKit platform once and reuse the result for every runtime platform.
# Base: node:24.21.0-bookworm-slim, pinned by multi-platform index digest.
FROM --platform=$BUILDPLATFORM ${NODE_BASE_REPOSITORY}@sha256:0e0ff40c39bc087845bfb27465a0df4ea419520094bc35842ff83dd8cbe6f9b6 AS builder

ENV PNPM_HOME=/pnpm \
    PATH=/pnpm:$PATH
WORKDIR /build

RUN corepack enable pnpm

# pnpm fetch is lockfile-only, so source edits no longer force package downloads.
# The subsequent filtered install materializes the web workspace from the cached
# store after the source tree is present (its prepare script runs svelte-kit sync).
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches ./patches
RUN pnpm fetch --frozen-lockfile

COPY packages/ui ./packages/ui
COPY apps/web ./apps/web
RUN pnpm install --offline --frozen-lockfile --filter @deepref/web... \
    && pnpm --filter @deepref/web build

FROM ${CADDY_BASE_REPOSITORY}@sha256:4c6e91c6ed0e2fa03efd5b44747b625fec79bc9cd06ac5235a779726618e530d AS runtime

ARG OCI_SOURCE="https://github.com/luzes-dev/deepref"
ARG OCI_REVISION="local"
ARG OCI_VERSION="0.0.0-dev"
ARG GIT_TREE_HASH="local"

LABEL org.opencontainers.image.source="${OCI_SOURCE}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.licenses="MIT" \
      dev.deepref.git.tree="${GIT_TREE_HASH}"

ENV HOME=/tmp \
    XDG_CONFIG_HOME=/config \
    XDG_DATA_HOME=/data

RUN mkdir -p /config/caddy /data/caddy \
    && chown -R 65532:65532 /config /data

COPY --chown=65532:65532 apps/web/Caddyfile /etc/caddy/Caddyfile
COPY --from=builder --chown=65532:65532 /build/apps/web/build /usr/share/caddy

USER 65532:65532
EXPOSE 8080
STOPSIGNAL SIGTERM
CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
