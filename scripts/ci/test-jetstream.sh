#!/usr/bin/env bash
# Hermetic, bounded, owned fixtures. Never connects to a developer database.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
lock_dir=${DEEPREF_IMPL_CACHE:-$HOME/.cache/deepref-impl}
mkdir -p "$lock_dir"
exec 9>"$lock_dir/cargo-build.lock"
flock -x 9
# Shared target builds must see the source of this checkout after lock ownership.
# Touch every Rust source in this checkout, including unchanged dependency
# crates. The shared target may otherwise reuse a sibling's newer enum/API.
find crates services apps tools \
  -type d \( -name node_modules -o -name target \) -prune -o \
  -type f -name '*.rs' -exec touch {} +
# This focused set uses small private DB clones. It is deliberately not the
# broad suite runner (which needs an explicitly sized 14 GiB filesystem).
available_kib=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
if (( available_kib < 6291456 )); then echo 'Need 6 GiB available memory for bounded fixtures' >&2; exit 1; fi
suffix="${USER:-ci}-$$-${RANDOM}"
pg="deepref-js-test-pg-$suffix"
nats="deepref-js-test-nats-$suffix"
usage() {
  docker inspect "$pg" --format 'owned PG memory={{.HostConfig.Memory}} tmpfs={{json .HostConfig.Tmpfs}}' 2>/dev/null || true
  docker exec "$pg" df -B1 /var/lib/postgresql/data 2>/dev/null || true
  docker exec "$pg" psql -U postgres -d deepref -Atc "SELECT datname,pg_database_size(oid) FROM pg_database ORDER BY datname" 2>/dev/null || true
}
cleanup() {
  usage
  docker unpause "$nats" >/dev/null 2>&1 || true
  docker rm -f "$nats" "$pg" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM
# Pinned image identities; CI may pre-pull these. Services bind loopback only.
docker run -d --name "$pg" --label deepref.test=jetstream --memory=4g --cpus=2 \
  --tmpfs /var/lib/postgresql/data:rw,size=3221225472 \
  -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=fixture-only -e POSTGRES_DB=deepref \
  -p 127.0.0.1::5432 \
  pgvector/pgvector@sha256:ac08538c6f8b9904c33c8224c5e5706dbe760aca29db1d096972b4052c22a75d \
  postgres -c max_connections=300 -c shared_buffers=64MB >/dev/null
docker run -d --name "$nats" --label deepref.test=jetstream --memory=256m --cpus=1 \
  --tmpfs /data:rw,size=134217728 -p 127.0.0.1::4222 -p 127.0.0.1::8222 \
  nats@sha256:ecf677bae6a0ae7900bd3217be041c6614d5dcd2cae780000f9cd69462b36541 \
  -js -sd /data -m 8222 >/dev/null
# The image's temporary initialization server accepts Unix sockets only and
# stops before the final server starts. Tests connect over TCP, so wait for
# that server rather than accepting the temporary socket and racing its exit.
pg_ready=false
for ((attempt=0;attempt<45;attempt++)); do
  if docker exec "$pg" pg_isready -h 127.0.0.1 -U postgres -d deepref >/dev/null 2>&1; then
    pg_ready=true
    break
  fi
  sleep 1
done
[[ "$pg_ready" == true ]] || { echo 'PostgreSQL TCP readiness failed before deadline' >&2; exit 1; }
pg_port=$(docker port "$pg" 5432/tcp | awk -F: '{print $NF}')
nats_port=$(docker port "$nats" 4222/tcp | awk -F: '{print $NF}')
monitor_port=$(docker port "$nats" 8222/tcp | awk -F: '{print $NF}')
# The pinned broker is a scratch image: readiness runs on the host, not via
# an assumed shell/curl inside the container. Require JetStream explicitly.
python3 scripts/ci/wait-jetstream.py "$monitor_port"
export DATABASE_URL="postgres://postgres:fixture-only@127.0.0.1:$pg_port/deepref"
export TEST_DATABASE_URL="$DATABASE_URL" DEEPREF_REQUIRE_DATABASE=1 DEEPREF_REQUIRE_NATS=1
export NATS_TEST_URL="nats://127.0.0.1:$nats_port" NATS_TEST_CONTAINER="$nats"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 SQLX_OFFLINE=true
# Caller may select a shared target; no unbounded background Cargo processes.
if (( $# )); then "$@"; else
  cargo nextest run -p deepref-config --lib --locked -E 'test(nats::tests)'
  cargo nextest run -p deepref-worker --test jetstream_contract --locked --no-fail-fast
  cargo nextest run -p deepref-worker --test jetstream_review --locked --no-fail-fast
  cargo nextest run -p deepref-worker --test jetstream_outage --locked
fi
