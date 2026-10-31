#!/usr/bin/env bash
# Disposable TLS S3 protocol contract. No external service credentials required.
# Fixture documentation: https://github.com/adobe/S3Mock/tree/5.1.0
set -euo pipefail
fixture_root=$(mktemp -d)
fixture_name="deepref-s3-contract-$$"
fixture_id=""
cleanup() {
  if [[ -n "$fixture_id" ]]; then
    docker rm -f "$fixture_id" >/dev/null 2>&1 || true
  fi
  rm -rf "$fixture_root"
}
trap cleanup EXIT
fixture_image=adobe/s3mock:5.1.0@sha256:65cf60155a2e235fe7d5bf6c633747d6fc7ed93f9f5a6727d86470026b83c2a2
fixture_id=$(docker run -d --name "$fixture_name" --label deepref.fixture=object-storage-contract --memory=512m --cpus=2 \
  -e COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS=deepref-contract \
  -p 127.0.0.1::9191 "$fixture_image")
fixture_port=$(docker port "$fixture_name" 9191/tcp | awk -F: '{print $NF}')
ready=0
for _ in $(seq 1 60); do
  if curl --insecure --silent --fail "https://localhost:$fixture_port/favicon.ico" >/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  docker logs "$fixture_name" >&2
  exit 1
fi
# Trust only the certificate served by this explicitly owned loopback fixture.
openssl s_client -connect "127.0.0.1:$fixture_port" -servername localhost -showcerts </dev/null 2>/dev/null \
  | openssl x509 -outform PEM > "$fixture_root/fixture-ca.pem"
export DEEPREF_S3_CONTRACT_ENDPOINT="https://localhost:$fixture_port"
export DEEPREF_S3_CONTRACT_CA="$fixture_root/fixture-ca.pem"
export DEEPREF_REQUIRE_S3_CONTRACT=1
cargo nextest run -p deepref-documents --test object_store_contract --locked --retries 0
