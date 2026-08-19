ARG RUST_BASE_REPOSITORY="docker.io/library/rust"
ARG DEBIAN_BASE_REPOSITORY="docker.io/library/debian"

FROM ${RUST_BASE_REPOSITORY}@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder

ARG PACKAGE
ARG BIN

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY apps ./apps
COPY crates ./crates
COPY services ./services

RUN test -n "${PACKAGE}" \
    && test -n "${BIN}" \
    && cargo build --release --locked --package "${PACKAGE}" --bin "${BIN}" \
    && strip "/build/target/release/${BIN}"

FROM ${DEBIAN_BASE_REPOSITORY}@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS api

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

FROM ${DEBIAN_BASE_REPOSITORY}@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS worker

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
