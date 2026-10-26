#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
mkdir -p "$HOME/.cache/deepref-impl"
exec 9>"$HOME/.cache/deepref-impl/cargo-build.lock"
flock -x -w 1800 9
rg --files crates services tools apps/server -g '*.rs' -0 | xargs -0 touch
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(dirname "$root")/shared-target}" CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 SQLX_OFFLINE=true
(( $(awk '/MemAvailable:/ {print $2}' /proc/meminfo) >= 6291456 )) || { echo 'fixture memory guard'; exit 1; }
fixture_id=''
cleanup() {
 for id in "$fixture_id"; do
  [[ -z "$id" ]] && continue
  [[ $(docker inspect --format '{{ index .Config.Labels "deepref.task" }}' "$id") == deepref-account-export-verification ]] || return 1
  docker inspect "$id" --format 'id={{.Id}} memory={{.HostConfig.Memory}} tmpfs={{json .HostConfig.Tmpfs}}'
  if [[ "$id" == "$fixture_id" ]]; then
   docker exec "$id" psql -U postgres -d full11_astra -Atc 'SHOW fsync; SHOW full_page_writes; SHOW synchronous_commit; SHOW max_connections; SELECT datname, pg_database_size(oid) FROM pg_database ORDER BY datname;'
   docker exec "$id" df -B1 /var/lib/postgresql/data
  fi
  docker rm -fv "$id"
 done
}
trap cleanup EXIT

fixture_id=$(docker run -d --label deepref.task=deepref-account-export-verification --memory=4g --cpus=2 --tmpfs /var/lib/postgresql/data:rw,size=3g -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=full11_astra -p 127.0.0.1::5432 pgvector/pgvector@sha256:ac08538c6f8b9904c33c8224c5e5706dbe760aca29db1d096972b4052c22a75d postgres -c max_connections=300 -c shared_buffers=64MB)
ready=false
for _ in $(seq 1 45); do
 if docker exec "$fixture_id" pg_isready -h 127.0.0.1 -U postgres -d full11_astra >/dev/null 2>&1 && [[ $(docker exec "$fixture_id" psql -h 127.0.0.1 -U postgres -d full11_astra -Atc 'SELECT 1') == 1 ]]; then ready=true; break; fi
 sleep 1
done
[[ "$ready" == true ]] || exit 1
pg_mapping=$(docker port "$fixture_id" 5432/tcp)
export DATABASE_URL="postgres://postgres:postgres@127.0.0.1:${pg_mapping##*:}/full11_astra" TEST_DATABASE_URL="postgres://postgres:postgres@127.0.0.1:${pg_mapping##*:}/full11_astra" DEEPREF_REQUIRE_DATABASE=1
docker exec "$fixture_id" psql -U postgres -d full11_astra -Atc 'SHOW fsync; SHOW full_page_writes; SHOW synchronous_commit;'
export EXPORT_FIXTURE_ID="$fixture_id"
if (( $# )); then
 "$@"
else
 cargo check --locked -p deepref-app -p deepref-http-api -p deepref-worker -p deepref-server
 cargo nextest run --locked -p deepref-app --test account_export_postgres -p deepref-http-api --test account_export --test authz_route_registration --test authz_user_scoped
 cargo nextest run --locked -p deepref-app --lib -E 'test(account_export::)'
fi
