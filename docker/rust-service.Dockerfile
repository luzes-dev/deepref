ARG RUST_BASE_REPOSITORY="docker.io/library/rust"
ARG DEBIAN_BASE_REPOSITORY="docker.io/library/debian"

FROM ${RUST_BASE_REPOSITORY}@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder

ARG OCI_REVISION="local"
ARG GIT_TREE_HASH="local"
ENV DEEPREF_BUILD_SHA="${OCI_REVISION}:${GIT_TREE_HASH}"
ENV SQLX_OFFLINE="true"

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx
COPY apps ./apps
COPY crates ./crates
COPY review-definitions ./review-definitions
COPY services ./services
COPY tools ./tools

# Both Rust service images share the same composition-root binary and select their
# runtime role through the container command.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --locked --package deepref-server \
    && strip /build/target/release/deepref-server

FROM ${DEBIAN_BASE_REPOSITORY}@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS pdfium

ARG PDFIUM_VERSION="7881"
ARG PDFIUM_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-linux-x64.tgz"
ARG PDFIUM_SHA256="1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && curl --fail --location --silent --show-error "${PDFIUM_URL}" --output /tmp/pdfium.tgz \
    && echo "${PDFIUM_SHA256}  /tmp/pdfium.tgz" | sha256sum --check --strict \
    && mkdir -p "/opt/pdfium/${PDFIUM_VERSION}" \
    && tar -xzf /tmp/pdfium.tgz -C "/opt/pdfium/${PDFIUM_VERSION}" lib/libpdfium.so LICENSE VERSION \
    && grep --fixed-strings --line-regexp "BUILD=${PDFIUM_VERSION}" "/opt/pdfium/${PDFIUM_VERSION}/VERSION" \
    && rm -f /tmp/pdfium.tgz

FROM ${DEBIAN_BASE_REPOSITORY}@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS runtime

ARG PDFIUM_VERSION="7881"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
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
