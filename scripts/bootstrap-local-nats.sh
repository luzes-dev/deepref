#!/usr/bin/env bash
set -euo pipefail

nats_url=${NATS_URL:-nats://127.0.0.1:4222}

request() {
  local subject=$1
  local payload=$2
  local response
  response=$(nats --server "$nats_url" request --raw --timeout 20s "$subject" "$payload")
  jq -e 'if .error then error(.error.description) else . end' <<<"$response" >/dev/null
}

upsert_stream() {
  local name=$1
  local payload=$2
  local action=CREATE
  if nats --server "$nats_url" stream info "$name" --json >/dev/null 2>&1; then
    action=UPDATE
  fi
  request "\$JS.API.STREAM.${action}.${name}" "$payload"
}

upsert_stream DEEPREF_WORK \
  '{"name":"DEEPREF_WORK","subjects":["work.fetch.requested.v1"],"retention":"workqueue","storage":"file","discard":"old","num_replicas":1,"duplicate_window":120000000000}'
upsert_stream DEEPREF_DOMAIN \
  '{"name":"DEEPREF_DOMAIN","subjects":["domain.>","projection.>"],"retention":"limits","storage":"file","discard":"old","max_age":2592000000000000,"num_replicas":1,"duplicate_window":120000000000}'
upsert_stream DEEPREF_DLQ \
  '{"name":"DEEPREF_DLQ","subjects":["dlq.recorded.v1"],"retention":"limits","storage":"file","discard":"old","max_age":7776000000000000,"num_replicas":1,"duplicate_window":120000000000}'

request '$JS.API.CONSUMER.DURABLE.CREATE.DEEPREF_WORK.deepref-worker' \
  '{"stream_name":"DEEPREF_WORK","config":{"durable_name":"deepref-worker","name":"deepref-worker","deliver_policy":"all","ack_policy":"explicit","ack_wait":1800000000000,"max_deliver":5,"backoff":[5000000000,30000000000,120000000000,600000000000,1800000000000],"filter_subject":"work.fetch.requested.v1","replay_policy":"instant","max_ack_pending":1024}}'
request '$JS.API.CONSUMER.DURABLE.CREATE.DEEPREF_DOMAIN.deepref-projector' \
  '{"stream_name":"DEEPREF_DOMAIN","config":{"durable_name":"deepref-projector","name":"deepref-projector","deliver_policy":"all","ack_policy":"explicit","ack_wait":1800000000000,"max_deliver":5,"backoff":[5000000000,30000000000,120000000000,600000000000,1800000000000],"filter_subject":"domain.>","replay_policy":"instant","max_ack_pending":1024}}'

printf 'local JetStream resources are ready at %s\n' "$nats_url"
