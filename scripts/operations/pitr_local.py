#!/usr/bin/env python3
"""Real local PostgreSQL WAL-target drill on exact new disposable fixtures.

No DATABASE_URL or existing database/container is accepted. This proves only
local backup/WAL mechanics, never a full customer restore, object agreement,
privacy replay, hosting egress policy, production durability, RPO or RTO.
The script owns its nonblocking global fixture flock; never wrap another flock.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid

from fault_proxy import FIXTURE_LOCK, require_held_lock

IMAGE = "pgvector/pgvector@sha256:ac08538c6f8b9904c33c8224c5e5706dbe760aca29db1d096972b4052c22a75d"
CACHE = Path.home() / ".cache/deepref-impl"
DATA_BYTES = 536870912
ARCHIVE_BYTES = 1073741824
MEMORY_BYTES = 2147483648
DISK_GUARD = 4294967296
MEMORY_GUARD_KIB = 6291456


class CapacityRefusal(RuntimeError):
    def __init__(self, message, exit_code):
        super().__init__(message)
        self.exit_code = exit_code


def bounded_identifier(value, pattern):
    if not isinstance(value, str) or not re.fullmatch(pattern, value):
        raise ValueError("unexpected fixture identifier")
    return value


def guard_capacity():
    if shutil.disk_usage(CACHE).free < DISK_GUARD:
        raise CapacityRefusal("unchanged 4GiB disk guard refused", 4)
    rows = dict(line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines())
    if int(rows["MemAvailable"].split()[0]) < MEMORY_GUARD_KIB:
        raise CapacityRefusal("6GiB available-memory guard refused", 5)


class Drill:
    def __init__(self, run, identity):
        self.run = run
        self.label = "operations-pitr-" + identity
        self.network_name = "deepref-ops-pitr-net-" + identity
        self.volume_name = "deepref-ops-pitr-data-" + identity
        self.network_id = None
        self.network_created = False
        self.volume_created = False
        self.containers = {}
        self.docker_endpoint = None
        self.sequence = 0
        self.password = "fixture-only"
        self.started = time.monotonic()
        self.result = {
            "schema_version": 1, "fixture_label": self.label,
            "image": IMAGE,
            "pitr_mechanics_verified": False, "full_restore_verified": False,
            "customer_restore_verified": False, "production_rpo_rto_verified": False,
            "application_started": False, "production_egress_tested": False,
            "negative_cases": [], "durations_seconds": {},
        }

    def command(self, label, args, timeout=15, input_text=None, check=True, env=None):
        self.sequence += 1
        path = self.run / f"{self.sequence:03d}-{label}.log"
        # Commands execute argv directly, never a host shell. Only fixture data.
        try:
            completed = subprocess.run(args, input=input_text, text=True,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                       timeout=timeout, check=False, env=env)
        except subprocess.TimeoutExpired as error:
            output = error.stdout or b""
            path.write_bytes(output if isinstance(output, bytes) else output.encode())
            raise RuntimeError("bounded fixture command timed out: " + label) from None
        if len(completed.stdout.encode()) > 2097152:
            path.write_text("fixture output exceeded 2MiB evidence bound\n")
            raise RuntimeError("fixture output bound exceeded: " + label)
        path.write_text(completed.stdout)
        if check and completed.returncode != 0:
            raise RuntimeError("fixture command failed: " + label)
        return completed

    def docker(self, label, *args, **kwargs):
        # Every API call uses the once-validated local endpoint. Inherited
        # context/host overrides cannot silently select another daemon.
        env = {k: v for k, v in os.environ.items() if k not in {"DOCKER_HOST", "DOCKER_CONTEXT"}}
        endpoint = ["--host", self.docker_endpoint] if self.docker_endpoint else []
        return self.command(label, ["docker", *endpoint, *args], env=env, **kwargs)

    def require_absent(self, kind, identifier):
        # A failed inspect alone is ambiguous with an unavailable daemon. A
        # successful scoped catalog query must also show no matching resource.
        selector = "id=" if re.fullmatch(r"[a-f0-9]{64}", identifier) and kind != "volume" else "name="
        args = [kind, "ls"]
        if kind == "container":
            args += ["--all", "--no-trunc"]
        elif kind == "network":
            args += ["--no-trunc"]
        args += ["--filter", selector + identifier, "--format", "{{.Name}}" if kind == "volume" else "{{.ID}}"]
        if self.docker("independent-absent-" + kind, *args).stdout.strip():
            raise RuntimeError("owned resource remains: " + kind)

    def inspect(self, kind, identifier):
        result = self.docker("inspect-" + kind, kind, "inspect", identifier)
        values = json.loads(result.stdout)
        if not isinstance(values, list) or len(values) != 1:
            raise RuntimeError("fixture catalog cardinality mismatch")
        value = values[0]
        labels = value.get("Labels") if kind != "container" else value.get("Config", {}).get("Labels")
        if not isinstance(labels, dict) or labels.get("deepref.task") != self.label:
            raise RuntimeError("fixture ownership label mismatch")
        return value

    def container(self, role):
        record = self.containers[role]
        value = self.inspect("container", record["id"])
        if value["Id"] != record["id"] or value["Name"] != "/" + record["name"]:
            raise RuntimeError("exact fixture identity mismatch")
        if value.get("HostConfig", {}).get("PortBindings"):
            raise RuntimeError("PITR fixture must have no published ports")
        host = value.get("HostConfig", {})
        if host.get("NetworkMode") != self.network_name:
            raise RuntimeError("PITR fixture network mismatch")
        if host.get("Memory") != MEMORY_BYTES or host.get("Tmpfs", {}).get("/var/lib/postgresql/data") != f"rw,size={DATA_BYTES}":
            raise RuntimeError("PITR fixture data/memory bounds mismatch")
        if value.get("Config", {}).get("Image") != IMAGE:
            raise RuntimeError("PITR fixture image mismatch")
        mounts = [m for m in value.get("Mounts", []) if m.get("Destination") == "/drill"]
        if len(mounts) != 1 or mounts[0].get("Name") != self.volume_name or mounts[0].get("RW") != (role == "source"):
            raise RuntimeError("PITR fixture archive mount mismatch")
        return value

    def exec(self, role, label, *args, timeout=15, check=True):
        self.container(role)
        return self.docker(label, "exec", "-e", "PGPASSWORD=" + self.password,
                           self.containers[role]["id"], *args, timeout=timeout, check=check)

    def query(self, role, sql, check=True):
        return self.exec(role, "query-" + role, "env",
                         "PGOPTIONS=-c statement_timeout=5000 -c lock_timeout=250",
                         "psql", "-XqAt", "-h", "127.0.0.1", "-U", "postgres",
                         "-d", "restore_probe", "-v", "ON_ERROR_STOP=1", "-c", sql,
                         timeout=8, check=check)

    def create_resources(self):
        context = os.environ.get("DOCKER_CONTEXT")
        if context:
            bounded_identifier(context, r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,79}")
            endpoint = self.docker("local-docker-context", "context", "inspect", context,
                                   "--format", "{{.Endpoints.docker.Host}}").stdout.strip()
        else:
            endpoint = os.environ.get("DOCKER_HOST") or self.docker(
                "local-docker-endpoint", "context", "inspect", "--format", "{{.Endpoints.docker.Host}}"
            ).stdout.strip()
        if not re.fullmatch(r"unix:///[^\x00\r\n]+", endpoint):
            raise RuntimeError("local Unix Docker fixture endpoint required")
        self.docker_endpoint = endpoint
        self.result["docker_endpoint_kind"] = "local_unix"
        self.docker("image-present", "image", "inspect", "--format", "{{.Id}}", IMAGE)
        self.network_created = True
        self.network_id = bounded_identifier(self.docker(
            "create-network", "network", "create", "--internal", "--driver", "bridge",
            "--label", "deepref.task=" + self.label, self.network_name
        ).stdout.strip(), r"[a-f0-9]{64}")
        network = self.inspect("network", self.network_id)
        if network["Name"] != self.network_name or not network.get("Internal"):
            raise RuntimeError("private internal network contract failed")
        # Record intent before creation so cleanup can resolve only this exact
        # name+label if Docker completed creation but its reply was interrupted.
        self.volume_created = True
        self.docker("create-volume", "volume", "create", "--driver", "local",
                    "--opt", "type=tmpfs", "--opt", "device=tmpfs",
                    "--opt", f"o=size={ARCHIVE_BYTES},mode=0700",
                    "--label", "deepref.task=" + self.label, self.volume_name)
        volume = self.inspect("volume", self.volume_name)
        if volume["Name"] != self.volume_name or volume.get("Driver") != "local" or volume.get("Options") != {
            "type": "tmpfs", "device": "tmpfs", "o": f"size={ARCHIVE_BYTES},mode=0700"
        }:
            raise RuntimeError("bounded archive volume contract failed")

    def start(self, role, restore_target=None, omit_wal=None):
        guard_capacity()
        name = "deepref-ops-pitr-" + role + "-" + self.label.removeprefix("operations-pitr-")
        cidfile = self.run / (role + ".cid")
        self.containers[role] = {"id": None, "name": name, "cidfile": str(cidfile)}
        flags = ["-c", "max_connections=300", "-c", "shared_buffers=64MB",
                 "-c", "max_wal_size=256MB", "-c", "listen_addresses=127.0.0.1"]
        if restore_target is None:
            # Keep the bounded tmpfs archive volume mounted after an explicit
            # clean PG stop, until all recovery instances have finished. A
            # source-only Docker stop would unmount the last tmpfs reference.
            script = ('mkdir -p /drill/wal; chown postgres:postgres /drill /drill/wal; '
                      'chmod 700 /drill /drill/wal; '
                      'docker-entrypoint.sh postgres "$@" & child=$!; '
                      "trap 'kill -TERM \"$child\"; wait \"$child\"' TERM INT; "
                      'wait "$child"; exec sleep 300')
            # Atomic publication, compare existing bytes, and sync before ACK.
            archive = ('dest="/drill/wal/%f"; if test -f "$dest"; then cmp "%p" "$dest"; '
                       'else cp "%p" "$dest.partial" && sync "$dest.partial" && '
                       'ln "$dest.partial" "$dest" && rm "$dest.partial"; fi')
            flags += ["-c", "archive_mode=on", "-c", "archive_command=" + archive]
        else:
            bounded_identifier(restore_target, r"drill_[a-f0-9]{32}|absent_[a-f0-9]{32}")
            script = ('test -f /drill/base/backup_manifest; cp -a /drill/base/. "$PGDATA/"; '
                      'test ! -e "$PGDATA/postmaster.pid"; touch "$PGDATA/recovery.signal"; '
                      'exec docker-entrypoint.sh postgres "$@"')
            restore = 'cp "/drill/wal/%f" "%p"'
            if omit_wal:
                bounded_identifier(omit_wal, r"[A-F0-9]{24}")
                restore = f'test "%f" != "{omit_wal}" && ' + restore
            flags += ["-c", "archive_mode=off", "-c", "restore_command=" + restore,
                      "-c", "recovery_target_name=" + restore_target,
                      "-c", "recovery_target_action=promote"]
        self.docker("start-" + role, "run", "-d", "--pull=never", "--cidfile", str(cidfile),
                    "--name", name, "--label", "deepref.task=" + self.label,
                    "--network", self.network_name, "--memory", str(MEMORY_BYTES),
                    "--cpus", "2", "--log-driver", "json-file", "--log-opt", "max-size=1m", "--log-opt", "max-file=1",
                    "--tmpfs", f"/var/lib/postgresql/data:rw,size={DATA_BYTES}",
                    "--mount", f"type=volume,src={self.volume_name},dst=/drill" + (",readonly" if restore_target else ""),
                    "-e", "POSTGRES_PASSWORD=" + self.password, "-e", "POSTGRES_DB=restore_probe",
                    "-e", "PGDATA=/var/lib/postgresql/data", "--entrypoint", "sh", IMAGE,
                    "-ceu", script, "--", *flags, timeout=30)
        self.containers[role]["id"] = bounded_identifier(cidfile.read_text().strip(), r"[a-f0-9]{64}")
        self.container(role)

    def wait_ready(self, role, seconds=45, require_recovery_complete=False):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            state = self.container(role)["State"]
            if not state["Running"]:
                raise RuntimeError("fixture exited before readiness: " + role)
            sql = "SELECT NOT pg_is_in_recovery()" if require_recovery_complete else "SELECT 1"
            expected = "t" if require_recovery_complete else "1"
            result = self.query(role, sql, check=False)
            if result.returncode == 0 and result.stdout.strip() == expected:
                return
            time.sleep(.2)
        raise RuntimeError("bounded fixture readiness failed: " + role)

    def settings(self, role):
        value = self.query(role, "SHOW fsync;SHOW full_page_writes;SHOW synchronous_commit;SHOW max_connections;SHOW wal_level").stdout.strip()
        if value != "on\non\non\n300\nreplica":
            raise RuntimeError("fixture default durability/configuration drift")
        return value

    def rejected_restore(self, role, target, omit_wal=None):
        started = time.monotonic()
        self.start(role, target, omit_wal)
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            state = self.container(role)["State"]
            if not state["Running"]:
                logs = self.docker("rejection-" + role, "logs", "--tail", "1000", self.containers[role]["id"]).stdout
                if state["ExitCode"] == 0 or "recovery ended before configured recovery target was reached" not in logs:
                    raise RuntimeError("restore failed outside intended target refusal: " + role)
                self.result["negative_cases"].append({"case": role, "target": target,
                    "omitted_wal": omit_wal, "backend_exit": state["ExitCode"],
                    "actual_target_refusal": True, "seconds": time.monotonic() - started})
                self.remove_container(role)
                return
            time.sleep(.2)
        raise RuntimeError("negative restore did not produce bounded actual backend refusal: " + role)

    def execute(self):
        self.create_resources()
        self.start("source")
        self.wait_ready("source")
        self.result["source_settings"] = self.settings("source")
        self.result["system_identifier"] = self.query("source", "SELECT system_identifier FROM pg_control_system()").stdout.strip()
        self.query("source", "CREATE TABLE restore_markers(marker text PRIMARY KEY,written_at timestamptz NOT NULL DEFAULT clock_timestamp());INSERT INTO restore_markers(marker) VALUES('A')")
        started = time.monotonic()
        self.exec("source", "basebackup", "gosu", "postgres", "pg_basebackup", "-h", "127.0.0.1", "-U", "postgres",
                  "--no-password", "--pgdata=/drill/base", "--checkpoint=fast", "--wal-method=stream",
                  "--manifest-checksums=SHA256", timeout=90)
        self.exec("source", "verify-backup", "gosu", "postgres", "pg_verifybackup", "/drill/base", timeout=45)
        self.result["durations_seconds"]["basebackup_and_verification"] = time.monotonic() - started
        self.exec("source", "backup-manifest", "cat", "/drill/base/backup_manifest")
        # Force a post-backup segment containing no included backup WAL. A
        # switch with no intervening WAL activity can be a no-op.
        self.query("source", "CHECKPOINT;SELECT pg_switch_wal()")
        self.query("source", "INSERT INTO restore_markers(marker) VALUES('B')")
        target = "drill_" + uuid.uuid4().hex
        before = self.query("source", "SELECT clock_timestamp() AT TIME ZONE 'UTC'").stdout.strip()
        lsn = self.query("source", "SELECT pg_create_restore_point('" + target + "')").stdout.strip()
        bounded_identifier(lsn, r"[0-9A-F]+/[0-9A-F]+")
        after = self.query("source", "SELECT clock_timestamp() AT TIME ZONE 'UTC'").stdout.strip()
        wal = self.query("source", "SELECT pg_walfile_name('" + lsn + "'::pg_lsn)").stdout.strip()
        bounded_identifier(wal, r"[0-9A-F]{24}")
        self.exec("source", "required-wal-not-in-base", "test", "!", "-e", "/drill/base/pg_wal/" + wal)
        self.result["target"] = {"name": target, "lsn": lsn, "utc_before": before, "utc_after": after, "required_wal": wal}
        self.query("source", "INSERT INTO restore_markers(marker) VALUES('C');SELECT pg_switch_wal()")
        deadline = time.monotonic() + 30
        while True:
            present = self.exec("source", "required-wal-ready", "test", "-f", "/drill/wal/" + wal, check=False)
            if present.returncode == 0:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("required actual archive segment missing")
            time.sleep(.2)
        required_hash = self.exec("source", "required-wal-sha256", "sha256sum", "/drill/wal/" + wal).stdout.split()[0]
        self.result["target"]["required_wal_sha256"] = bounded_identifier(required_hash, r"[a-f0-9]{64}")
        if self.query("source", "SELECT string_agg(marker,',' ORDER BY marker) FROM restore_markers").stdout.strip() != "A,B,C":
            raise RuntimeError("source markers incomplete")
        self.query("source", "SHOW fsync;SHOW full_page_writes;SHOW synchronous_commit;SHOW max_connections;SELECT datname,pg_database_size(oid) FROM pg_database ORDER BY datname")
        self.exec("source", "source-before-stop-df", "df", "-B1", "/var/lib/postgresql/data", "/drill")
        self.exec("source", "stop-source-writes", "gosu", "postgres", "pg_ctl", "-D", "/var/lib/postgresql/data", "-m", "fast", "-w", "-t", "15", "stop", timeout=20)
        if self.query("source", "SELECT 1", check=False).returncode == 0:
            raise RuntimeError("source PostgreSQL still accepts queries after stop")
        self.containers["source"]["postgres_stopped"] = True
        self.result["source_postgres_stopped_before_recovery"] = True
        catalog_command = 'for path in /drill/wal/*; do test -f "$path" && sha256sum "$path"; done'
        catalog_before = self.exec("source", "archive-catalog-before", "sh", "-ceu", catalog_command).stdout
        self.rejected_restore("missing-wal", target, wal)
        self.rejected_restore("missing-target", "absent_" + uuid.uuid4().hex)
        started = time.monotonic()
        self.start("recovered", target)
        self.wait_ready("recovered", require_recovery_complete=True)
        self.result["recovered_settings"] = self.settings("recovered")
        recovered_system = self.query("recovered", "SELECT system_identifier FROM pg_control_system()").stdout.strip()
        if recovered_system != self.result["system_identifier"]:
            raise RuntimeError("recovered cluster is not the backed-up system")
        actual = self.query("recovered", "SELECT pg_is_in_recovery();SELECT string_agg(marker,',' ORDER BY marker) FROM restore_markers;SELECT pg_last_wal_replay_lsn()").stdout.strip().splitlines()
        if len(actual) != 3 or actual[0] != "f" or actual[1] != "A,B":
            raise RuntimeError("recovered target/marker assertion failed")
        logs = self.docker("positive-recovery-log", "logs", "--tail", "1000", self.containers["recovered"]["id"]).stdout
        if "recovery stopping at restore point" not in logs or target not in logs:
            raise RuntimeError("actual named-target recovery log missing")
        self.result["durations_seconds"]["positive_recovery_validation"] = time.monotonic() - started
        self.result["recovered_marker_set"] = actual[1]
        self.result["replay_lsn"] = actual[2]
        catalog_after = self.exec("source", "archive-catalog-after", "sh", "-ceu", catalog_command).stdout
        if catalog_after != catalog_before:
            raise RuntimeError("archive changed during recovery")
        self.result["archive_catalog_sha256"] = hashlib.sha256(catalog_before.encode()).hexdigest()
        self.result["pitr_mechanics_verified"] = True

    def remove_container(self, role):
        record = self.containers[role]
        if record.get("removed"):
            return
        if record["id"] is None:
            cidfile = Path(record["cidfile"])
            if cidfile.exists():
                record["id"] = bounded_identifier(cidfile.read_text().strip(), r"[a-f0-9]{64}")
            else:
                inspected = self.docker("resolve-owned-" + role, "container", "inspect", record["name"], check=False)
                if inspected.returncode != 0:
                    self.require_absent("container", record["name"])
                    record["removed"] = True
                    return
                candidate = json.loads(inspected.stdout)[0]
                if candidate["Name"] != "/" + record["name"] or candidate["Config"]["Labels"].get("deepref.task") != self.label:
                    raise RuntimeError("cannot resolve exact owned fixture after interrupted create")
                record["id"] = bounded_identifier(candidate["Id"], r"[a-f0-9]{64}")
        value = self.container(role)
        self.docker("final-log-" + role, "logs", "--tail", "1000", record["id"])
        if value["State"]["Running"]:
            if not record.get("postgres_stopped"):
                self.query(role, "SHOW fsync;SHOW full_page_writes;SHOW synchronous_commit;SHOW max_connections;SELECT datname,pg_database_size(oid) FROM pg_database ORDER BY datname", check=False)
            self.exec(role, "tmpfs-df-" + role, "df", "-B1", "/var/lib/postgresql/data", "/drill")
        anonymous = [m["Name"] for m in value.get("Mounts", []) if m["Type"] == "volume" and m["Name"] != self.volume_name]
        self.docker("remove-" + role, "rm", "-fv", record["id"])
        if self.docker("absent-" + role, "container", "inspect", record["id"], check=False).returncode == 0:
            raise RuntimeError("owned container remains")
        self.require_absent("container", record["id"])
        for volume in anonymous:
            if self.docker("anonymous-volume-absent", "volume", "inspect", volume, check=False).returncode == 0:
                raise RuntimeError("owned anonymous volume remains")
            self.require_absent("volume", volume)
        record["removed"] = True

    def cleanup(self):
        errors = []
        for role in reversed(list(self.containers)):
            try:
                self.remove_container(role)
            except (RuntimeError, ValueError, OSError, KeyError) as error:
                errors.append(type(error).__name__ + ":" + role)
        if self.volume_created:
            try:
                value = self.inspect("volume", self.volume_name)
                if value["Name"] != self.volume_name:
                    raise RuntimeError("volume identity mismatch")
                self.docker("remove-volume", "volume", "rm", self.volume_name)
                if self.docker("volume-absent", "volume", "inspect", self.volume_name, check=False).returncode == 0:
                    raise RuntimeError("owned archive volume remains")
                self.require_absent("volume", self.volume_name)
            except (RuntimeError, ValueError, OSError, KeyError):
                errors.append("owned-volume")
        if self.network_created:
            try:
                value = self.inspect("network", self.network_id or self.network_name)
                self.network_id = bounded_identifier(value["Id"], r"[a-f0-9]{64}")
                if value["Name"] != self.network_name or value.get("Containers"):
                    raise RuntimeError("owned network still occupied or identity mismatch")
                self.docker("remove-network", "network", "rm", self.network_id)
                if self.docker("network-absent", "network", "inspect", self.network_id, check=False).returncode == 0:
                    raise RuntimeError("owned internal network remains")
                self.require_absent("network", self.network_id)
            except (RuntimeError, ValueError, OSError, KeyError):
                errors.append("owned-network")
        self.result["cleanup"] = {"errors": errors, "exact_containers": self.containers,
                                  "network_id": self.network_id, "archive_volume": self.volume_name}
        return not errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--parent-routed", action="store_true", required=True)
    args = parser.parse_args()
    if not args.parent_routed:
        parser.error("requires explicit actual parent route, not just this accidental-run guard")
    with open(FIXTURE_LOCK, "a+b") as lock:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            parser.exit(3, "fixture flock busy; no queue\n")
        require_held_lock(FIXTURE_LOCK, lock.fileno())
        try:
            guard_capacity()
        except CapacityRefusal as error:
            parser.exit(error.exit_code, str(error) + "; no fixture created\n")
        identity = uuid.uuid4().hex
        run = CACHE / ("operations-local-pitr-" + identity)
        run.mkdir(mode=0o700)
        source = Path(__file__)
        sources = [source, source.with_name("fault_proxy.py")]
        source_hashes = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in sources}
        source_hash = source_hashes[str(source)]
        drill = Drill(run, identity)
        result = 1

        def interrupted(number, _frame):
            raise KeyboardInterrupt("owned drill interrupted")

        for number in (signal.SIGINT, signal.SIGTERM):
            signal.signal(number, interrupted)
        print(json.dumps({"run": str(run), "source_sha256": source_hash}), flush=True)
        try:
            drill.execute()
            result = 0
        except CapacityRefusal as error:
            result = error.exit_code
            drill.result["failure"] = {"kind": type(error).__name__, "message": str(error)}
        except (RuntimeError, ValueError, OSError, KeyError, KeyboardInterrupt) as error:
            drill.result["failure"] = {"kind": type(error).__name__, "message": str(error)}
        finally:
            # Once interrupted, finish exact bounded cleanup rather than let a
            # second signal strand task-owned resources.
            for number in (signal.SIGINT, signal.SIGTERM):
                signal.signal(number, signal.SIG_IGN)
            if not drill.cleanup():
                result = 125 if result == 0 else result
            if any(hashlib.sha256(p.read_bytes()).hexdigest() != source_hashes[str(p)] for p in sources):
                drill.result["source_changed_during_drill"] = True
                result = 126
            drill.result["source_sha256"] = source_hash
            drill.result["source_hashes"] = source_hashes
            drill.result["duration_seconds"] = time.monotonic() - drill.started
            drill.result["exit"] = result
            drill.result["local_drill_complete"] = result == 0 and drill.result["pitr_mechanics_verified"]
            (run / "result.json").write_text(json.dumps(drill.result, indent=2) + "\n")
            (run / "final-disk.json").write_text(json.dumps(shutil.disk_usage(CACHE)._asdict()) + "\n")
        return result


if __name__ == "__main__":
    sys.exit(main())
