FROM rust:1.95-bookworm AS builder

ARG PACKAGE=deepref-api
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY services ./services

RUN cargo build --release --locked -p "${PACKAGE}"

FROM debian:bookworm-slim AS runtime

ARG BIN=deepref-api
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/${BIN} /usr/local/bin/deepref-service

ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/deepref-service"]
