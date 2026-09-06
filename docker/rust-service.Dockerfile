ARG RUST_BASE_REPOSITORY="docker.io/library/rust"
ARG DEBIAN_BASE_REPOSITORY="docker.io/library/debian"
ARG CARGO_CHEF_VERSION="0.1.78"
ARG SCCACHE_VERSION="0.17.0"

FROM ${RUST_BASE_REPOSITORY}@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS toolchain

WORKDIR /build

# Use checksum-pinned release binaries instead of compiling the build tooling in
# every cold Docker cache. The Rust image is buildpack-deps based and already
# provides curl, tar, xz and CA certificates.
ARG TARGETARCH
ARG CARGO_CHEF_VERSION
ARG SCCACHE_VERSION
ARG CARGO_CHEF_SHA256_AMD64="70ef940ef90d04d122f0176fdb8d6c39069191b484a1eaa29b327370c2e1c3c0"
ARG CARGO_CHEF_SHA256_ARM64="a47e13fba89c2895f5a5c3d0844acd2a5fd416eceb3a6f9dfb26e28155099f4e"
ARG SCCACHE_SHA256_AMD64="67c4a96dd237c1f518f6b36083f270f9976d516f1e57fce891755ea782e50006"
ARG SCCACHE_SHA256_ARM64="821a86343191aa1cbab74bd42f9e93c9a63bf85e4742945f40d3ae84193c1c77"
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) \
        cargo_chef_target="x86_64-unknown-linux-gnu"; \
        cargo_chef_sha256="${CARGO_CHEF_SHA256_AMD64}"; \
        sccache_target="x86_64-unknown-linux-musl"; \
        sccache_sha256="${SCCACHE_SHA256_AMD64}" \
        ;; \
      arm64) \
        cargo_chef_target="aarch64-unknown-linux-gnu"; \
        cargo_chef_sha256="${CARGO_CHEF_SHA256_ARM64}"; \
        sccache_target="aarch64-unknown-linux-musl"; \
        sccache_sha256="${SCCACHE_SHA256_ARM64}" \
        ;; \
      *) echo "unsupported target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl --fail --location --silent --show-error \
      "https://github.com/LukeMathWalker/cargo-chef/releases/download/v${CARGO_CHEF_VERSION}/cargo-chef-${cargo_chef_target}.tar.xz" \
      --output /tmp/cargo-chef.tar.xz; \
    echo "${cargo_chef_sha256}  /tmp/cargo-chef.tar.xz" | sha256sum --check --strict; \
    mkdir -p /tmp/cargo-chef; \
    tar -xJf /tmp/cargo-chef.tar.xz -C /tmp/cargo-chef; \
    find /tmp/cargo-chef -type f -name cargo-chef -exec install -m 0755 {} /usr/local/bin/cargo-chef \;; \
    curl --fail --location --silent --show-error \
      "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-${sccache_target}.tar.gz" \
      --output /tmp/sccache.tar.gz; \
    echo "${sccache_sha256}  /tmp/sccache.tar.gz" | sha256sum --check --strict; \
    mkdir -p /tmp/sccache; \
    tar -xzf /tmp/sccache.tar.gz -C /tmp/sccache; \
    find /tmp/sccache -type f -name sccache -exec install -m 0755 {} /usr/local/bin/sccache \;; \
    cargo-chef --version; \
    sccache --version; \
    rm -rf /tmp/cargo-chef /tmp/cargo-chef.tar.xz /tmp/sccache /tmp/sccache.tar.gz

FROM toolchain AS chef

# Keep the repository-pinned toolchain visible to cargo-chef and Cargo.
COPY rust-toolchain.toml ./

FROM chef AS planner

# The planner needs all manifests and source layout, including future tooling
# crates, to produce a complete workspace recipe. .dockerignore keeps caches
# and local secrets out of this context.
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

ARG OCI_REVISION="local"
ARG GIT_TREE_HASH="local"
ENV DEEPREF_BUILD_SHA="${OCI_REVISION}:${GIT_TREE_HASH}" \
    SQLX_OFFLINE="true" \
    RUSTC_WRAPPER="sccache" \
    SCCACHE_DIR="/sccache"

COPY --from=planner /build/recipe.json recipe.json

# Build third-party dependencies before application source is copied. The GHA
# layer cache handles identical recipes; the persisted sccache mount salvages
# compilation work when a layer is invalidated but compiler inputs are unchanged.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=deepref-sccache,target=/sccache,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json --locked

COPY . .

# Both Rust service images share the same composition-root binary and select their
# runtime role through the container command.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=deepref-sccache,target=/sccache,sharing=locked \
    cargo build --release --locked --package deepref-server \
    && strip /build/target/release/deepref-server

# Download the PDFium artifact on the native build platform. This avoids an
# apt/curl bootstrap in a separate Debian stage and, critically, selects the
# correct shared library for the target architecture.
FROM toolchain AS pdfium

ARG TARGETARCH
ARG PDFIUM_VERSION="7881"
ARG PDFIUM_SHA256_AMD64="1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d"
ARG PDFIUM_SHA256_ARM64="ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674"
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) pdfium_asset="pdfium-linux-x64.tgz"; pdfium_sha256="${PDFIUM_SHA256_AMD64}" ;; \
      arm64) pdfium_asset="pdfium-linux-arm64.tgz"; pdfium_sha256="${PDFIUM_SHA256_ARM64}" ;; \
      *) echo "unsupported target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl --fail --location --silent --show-error \
      "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_VERSION}/${pdfium_asset}" \
      --output /tmp/pdfium.tgz; \
    echo "${pdfium_sha256}  /tmp/pdfium.tgz" | sha256sum --check --strict; \
    mkdir -p "/opt/pdfium/${PDFIUM_VERSION}"; \
    tar -xzf /tmp/pdfium.tgz -C "/opt/pdfium/${PDFIUM_VERSION}" lib/libpdfium.so LICENSE VERSION; \
    grep --fixed-strings --line-regexp "BUILD=${PDFIUM_VERSION}" "/opt/pdfium/${PDFIUM_VERSION}/VERSION"; \
    rm -f /tmp/pdfium.tgz

FROM ${DEBIAN_BASE_REPOSITORY}@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS runtime

ARG PDFIUM_VERSION="7881"
COPY --from=pdfium "/opt/pdfium/${PDFIUM_VERSION}/lib/libpdfium.so" /usr/local/lib/libpdfium.so

ENV PDFIUM_LIBRARY_PATH=/usr/local/lib/libpdfium.so

FROM runtime AS api

ARG OCI_SOURCE="https://github.com/luzes-dev/deepref"
ARG OCI_REVISION="local"
ARG OCI_VERSION="0.0.0-dev"
ARG GIT_TREE_HASH="local"

LABEL org.opencontainers.image.source="${OCI_SOURCE}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.licenses="MIT" \
      dev.deepref.git.tree="${GIT_TREE_HASH}"

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder --chown=65532:65532 /build/target/release/deepref-server /usr/local/bin/deepref-server

WORKDIR /app
ENV HOME=/tmp \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    RUST_LOG=info
USER 65532:65532
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/deepref-server"]
CMD ["serve"]

FROM runtime AS worker

ARG OCI_SOURCE="https://github.com/luzes-dev/deepref"
ARG OCI_REVISION="local"
ARG OCI_VERSION="0.0.0-dev"
ARG GIT_TREE_HASH="local"

LABEL org.opencontainers.image.source="${OCI_SOURCE}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.licenses="MIT" \
      dev.deepref.git.tree="${GIT_TREE_HASH}"

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder --chown=65532:65532 /build/target/release/deepref-server /usr/local/bin/deepref-server

WORKDIR /app
ENV HOME=/tmp \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    RUST_LOG=info
USER 65532:65532
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/deepref-server"]
CMD ["worker"]
