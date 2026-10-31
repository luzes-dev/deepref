#!/usr/bin/env bash
# Required real-service contract. Run under the caller's Cargo lock locally.
set -euo pipefail
: "${DATABASE_URL:?disposable PostgreSQL URL required}"
: "${TEST_DATABASE_URL:?matching disposable PostgreSQL URL required}"
[[ "$DATABASE_URL" == "$TEST_DATABASE_URL" ]] || { echo 'fixture database URLs must match' >&2; exit 1; }
export DEEPREF_REQUIRE_DATABASE=1 DEEPREF_REQUIRE_VALKEY=1
fixture_id=''
fixture_label="valkey-contract-$$"
cleanup() {
  if [[ -n "$fixture_id" ]]; then
    [[ $(docker inspect --format '{{ index .Config.Labels "deepref.fixture" }}' "$fixture_id") == "$fixture_label" ]] || return 1
    docker rm -fv "$fixture_id" >/dev/null
  fi
}
trap cleanup EXIT
fixture_id=$(docker run -d --label "deepref.fixture=$fixture_label" --memory=128m --cpus=1 \
  --tmpfs /data:rw,size=64m -p 127.0.0.1::6379 \
  valkey/valkey@sha256:70739f85ad2ee01a726a965584a0f94895f01b0c60b3cc8b0aeef11eaa6888cf \
  valkey-server --save '' --appendonly no --maxmemory 64mb --maxmemory-policy noeviction)
ready=false
for _ in $(seq 1 30); do
  if [[ $(docker exec "$fixture_id" valkey-cli ping 2>/dev/null) == PONG ]]; then ready=true; break; fi
  sleep 1
done
[[ "$ready" == true ]] || { echo 'Valkey fixture readiness failed' >&2; exit 1; }
mapping=$(docker port "$fixture_id" 6379/tcp)
export TEST_VALKEY_URL="redis://127.0.0.1:${mapping##*:}"
cargo nextest run -p deepref-application --lib --locked -E 'test(rate_limits::)' --retries 0
cargo nextest run -p deepref-config --lib --locked -E 'test(valkey::)' --retries 0
cargo nextest run -p deepref-app --test rate_limits --test valkey_limits --locked --retries 0
cargo nextest run -p deepref-http-api --test api_keys --locked --retries 0
