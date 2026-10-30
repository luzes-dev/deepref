#!/usr/bin/env python3
"""Explicitly routed, owned SDK/gRPC capture gate; no external metrics backend.

Pin-only is read-only. Dependency/source activation is a separate prerequisite;
this runner never edits source while a compiler or receiver observes it.
"""

import argparse
from datetime import datetime, timezone
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import subprocess
import sys
import time
import tomllib

from fault_proxy import FIXTURE_LOCK, require_held_lock
from metrics_catalog import assess_panels, read_json_bounded
from otlp_snapshot import convert_capture
from test_observability import ROOT, guards, owned_closed, phase_group_inventory, run_phase

CASE = "otlp_capture::real_sdk_gauges_retain_source_time"
FEATURE = "operations-fixtures"
SERVICE = "operations-capture-fixture"
OUTER_SECONDS = 720
SOURCES = (
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "crates/telemetry/Cargo.toml",
    "crates/telemetry/src/lib.rs", "crates/telemetry/src/metrics.rs", "crates/telemetry/src/transport.rs",
    "crates/telemetry/src/otlp_capture.rs", "operations/metrics/catalog.json",
    "scripts/operations/otlp_snapshot.py", "scripts/operations/tests/test_otlp_snapshot.py",
    "scripts/operations/test_otlp_capture.py", "scripts/operations/test_observability.py",
    "scripts/operations/tests/test_otlp_capture_runner.py",
    "scripts/operations/tests/test_phase_lifetime.py",
    "scripts/operations/metrics_catalog.py", "scripts/operations/fault_proxy.py",
)
def pins():
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT, timeout=5).decode().split("\0")
    # Seal all tracked resources, including SQLx metadata and include_str!/bytes!
    # inputs, rather than assuming Rust/Cargo text alone describes compilation.
    names = sorted(set(SOURCES) | {name for name in tracked if name})
    return {"head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, timeout=5).strip(),
            "tree": subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=ROOT, text=True, timeout=5).strip(),
            "files": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in names}}


def package_ids():
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    return {(row["name"], row["version"], row.get("source", "workspace")) for row in lock["package"]}


def require_guards(value):
    if value["free_bytes"] < value["minimum_free_bytes"] or value["mem_available_bytes"] < value["minimum_mem_available_bytes"]:
        raise RuntimeError("unchanged disk/memory guard refused")


def owned_process(fixture, nonce, *, terminate):
    path = fixture / "owned-process.json"
    if not path.exists():
        return {"process_recorded": False, "running": False}
    row = json.loads(path.read_text())
    if row.get("nonce") != nonce or row.get("test") != CASE or type(row.get("pid")) is not int:
        raise ValueError("owned capture process identity mismatch")
    pid = row["pid"]
    proc = Path("/proc") / str(pid)

    def live():
        try:
            fields = (proc / "stat").read_text().rsplit(") ", 1)[1].split()
        except FileNotFoundError:
            return False
        if fields[19] != row["start_ticks"]:
            raise ValueError("owned capture PID reused")
        if fields[0] == "Z":
            return False
        env = (proc / "environ").read_bytes().split(b"\0")
        if ("DEEPREF_OTLP_RUN_NONCE=" + nonce).encode() not in env or ("DEEPREF_OTLP_CAPTURE_DIR=" + str(fixture)).encode() not in env:
            raise ValueError("owned capture environment mismatch")
        executable = (proc / "exe").resolve(strict=True)
        target = (ROOT.parent / "shared-target/debug/deps").resolve()
        if executable.parent != target or not executable.name.startswith("deepref_telemetry-"):
            raise ValueError("owned capture executable mismatch")
        return True

    initially_live = live()
    if initially_live and terminate:
        os.kill(pid, signal.SIGTERM)
        until = time.monotonic() + 1
        while time.monotonic() < until and live():
            time.sleep(0.02)
        if live():
            os.kill(pid, signal.SIGKILL)
            until = time.monotonic() + 1
            while time.monotonic() < until and live():
                time.sleep(0.02)
    return {"pid": pid, "start_ticks": row["start_ticks"], "process_recorded": True,
            "running": live(), "emergency_termination_required": initially_live and terminate}


def received_phase(fixture, phase):
    """Bind combined phase data to two distinct, retained received RPCs.

    Rust independently decodes its reencoded protobuf before writing either
    artifact. Python verifies the JSON assembly and hashes both retained files;
    hashing the protobuf here does not repeat that typed decoding proof.
    """
    sidecar = read_json_bounded(fixture / ("phase-" + phase + "-sources.json"))
    names = sidecar.get("assembled_from_received_rpc_files")
    if (not isinstance(names, list) or len(names) != 2 or len(set(names)) != 2
            or any(not isinstance(name, str) or not re.fullmatch(r"received-[0-9]{2}", name) for name in names)
            or sidecar.get("original_network_frames_retained") is not False
            or sidecar.get("gRPC_fixture_capture") is not True
            or sidecar.get("backend_ingestion_verified") is not False
            or sidecar.get("production_accepted") is not False):
        raise ValueError("phase lacks exact received-RPC provenance")
    resources = []
    files = {}
    for name in names:
        request = read_json_bounded(fixture / (name + ".json"))
        rows = request.get("resourceMetrics")
        if not isinstance(rows, list) or len(rows) != 1:
            raise ValueError("expected one resource per received RPC")
        resources.extend(rows)
        for suffix in (".json", ".reencoded.pb"):
            path = fixture / (name + suffix)
            if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 262144:
                raise ValueError("received artifact outside retained file bounds")
            data = path.read_bytes()
            files[path.name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    combined = read_json_bounded(fixture / ("phase-" + phase + ".json"))
    if combined != {"resourceMetrics": resources}:
        raise ValueError("phase differs from its received RPCs")
    return combined, files


def phase_assertions(fixture, *, now=None):
    now = datetime.now(timezone.utc) if now is None else now
    catalog = read_json_bounded(ROOT / "operations/metrics/catalog.json")
    assessed = {}
    for phase in ("stale", "fresh", "failed"):
        capture, files = received_phase(fixture, phase)
        snapshot, provenance = convert_capture(capture, catalog, service_name=SERVICE, now=now, repository=ROOT)
        if provenance["selected_processes"] != 2 or provenance["unbound_resources"] != 0:
            raise ValueError("phase requires two distinct owned SDK resources")
        panels = assess_panels(catalog, snapshot, now)
        queue = next(row for row in panels if row["id"] == "queue-age:pool=documents")
        transport = next(row for row in panels if row["id"] == "transport-observation")
        expected = ("observed", 7, 2) if phase == "fresh" else ("unknown", None, 0)
        if (queue["state"], queue["value"], queue["fresh_replicas"]) != expected:
            raise ValueError("actual SDK source-time/replica assertion failed: " + phase)
        if transport["state"] != "observed" or transport["value"] != (0 if phase == "failed" else 1):
            raise ValueError("actual transport observer gauge assertion failed")
        if provenance["source_capture_adapter_verified"] or provenance["backend_ingestion_verified"] or provenance["production_accepted"]:
            raise ValueError("generic file provenance cannot imply operational acceptance")
        (fixture / ("phase-" + phase + "-normalized.json")).write_text(json.dumps(snapshot, indent=2) + "\n")
        assessed[phase] = {"queue": {key: queue[key] for key in ("state", "value", "fresh_replicas", "rejected_replicas")},
                           "transport": {key: transport[key] for key in ("state", "value", "latest_capture_ms")},
                           "file_provenance": provenance, "received_artifacts": files}
    return assessed


def final_inventory(output, nonce, active, result):
    """Every independent cleanup observation runs even when another refuses."""
    errors = []

    def check(name, action):
        try:
            action()
        except BaseException as error:
            errors.append({"step": name, "type": type(error).__name__, "error": str(error)})

    if output is not None:
        def process_inventory():
            row = owned_process(output / "fixture", nonce, terminate=True)
            result["owned_process"] = row
            if (result["exit"] == 0 and not row["process_recorded"]
                    or row["running"] or row.get("emergency_termination_required")):
                raise RuntimeError("capture process absent from inventory or required emergency termination")

        def port_inventory():
            path = output / "fixture/owned-port.json"
            if not path.exists():
                if result["exit"] == 0:
                    raise RuntimeError("successful capture lacks its exact owned port")
                return
            row = json.loads(path.read_text())
            port = row.get("port")
            if (type(port) is not int or not 1 <= port <= 65535 or row.get("test") != CASE
                    or row.get("scope") != "local_metric_fixture"):
                raise ValueError("invalid owned port identity")
            result["exact_port"] = {"port": port, "independently_closed": owned_closed(port)}
            if not result["exact_port"]["independently_closed"]:
                raise RuntimeError("owned gRPC listener remains open")

        def temp_inventory():
            result["tmp_entries"] = [str(path.relative_to(output)) for path in (output / "tmp").rglob("*")]
            if result["tmp_entries"]:
                raise RuntimeError("capture temporary directory remains populated")

        check("process", process_inventory)
        check("port", port_inventory)
        check("temporary-files", temp_inventory)

    def source_inventory():
        result["source_after"] = pins()
        if result["source_after"] != active:
            raise RuntimeError("source changed during compilation or capture")

    def numeric_inventory():
        result["final_guards"] = guards()

    def phase_inventory():
        result["phase_groups"] = phase_group_inventory(result["phases"])
        if not all(row["closed"] for row in result["phase_groups"]):
            raise RuntimeError("owned phase process group remains")

    check("phase-groups", phase_inventory)
    check("source", source_inventory)
    check("numeric-guards", numeric_inventory)
    result["cleanup_errors"] = errors
    result["cleanup_exit"] = 1 if errors else 0


def execute(args):
    deadline = time.monotonic() + OUTER_SECONDS
    expected = json.loads(args.manifest.read_text())
    if expected != pins():
        raise ValueError("source pins changed before capture gate")
    result = {"phases": [], "exit": 1, "cleanup_exit": 1, "local_fixture_sdk_gRPC_capture": False,
              "backend_ingestion_verified": False, "production_accepted": False,
              "started_at": datetime.now(timezone.utc).isoformat(),
              "python_executable": sys.executable, "python_version": sys.version,
              "outer_budget_seconds": OUTER_SECONDS, "capture_command_budget_seconds": 65}
    output = None
    active = expected
    nonce = secrets.token_hex(16)
    with FIXTURE_LOCK.open("a+") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        alias = lock.fileno() != 9
        if alias:
            os.dup2(lock.fileno(), 9, inheritable=True)
        else:
            os.set_inheritable(9, True)
        try:
            require_held_lock(FIXTURE_LOCK, 9)
            result["initial_guards"] = guards()
            require_guards(result["initial_guards"])
            if expected != pins():
                raise ValueError("source pins changed at acquisition")
            args.output.mkdir(mode=0o700)
            output = args.output
            (output / "fixture").mkdir(mode=0o700)
            (output / "tmp").mkdir(mode=0o700)
            (output / "source-before.json").write_text(json.dumps(expected, indent=2) + "\n")
            env = {key: value for key, value in os.environ.items() if not key.startswith("OTEL_")
                   and key.lower() not in {"http_proxy", "https_proxy", "all_proxy", "no_proxy"}
                   and key not in {"DEEPREF_OTLP_SOURCE", "DEEPREF_CATALOG_SOURCE", "DATABASE_URL", "TEST_DATABASE_URL"}}
            env.update(CARGO_TARGET_DIR=str(ROOT.parent / "shared-target"), CARGO_PROFILE_DEV_DEBUG="0",
                       CARGO_PROFILE_TEST_DEBUG="0", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2", CARGO_NET_OFFLINE="true",
                       SQLX_OFFLINE="true", PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(output / "tmp"),
                       NO_PROXY="127.0.0.1,::1,localhost", DEEPREF_REQUIRE_OTLP_CAPTURE="1",
                       DEEPREF_OTLP_LOCK_OWNER_PID=str(os.getpid()), DEEPREF_OTLP_RUN_NONCE=nonce,
                       DEEPREF_OTLP_CAPTURE_DIR=str(output / "fixture"))

            def run(name, command, seconds=180):
                receipt = {}
                result["phases"].append(receipt)
                run_phase(name, command, output=output, environment=env,
                          deadline=min(deadline, time.monotonic() + seconds + 5), receipt=receipt)
                if receipt["exit"] != 0:
                    raise RuntimeError("phase failed before acceptance: " + name)

            manifest = tomllib.loads((ROOT / "crates/telemetry/Cargo.toml").read_text())
            if FEATURE not in manifest.get("features", {}):
                raise ValueError("test feature/dependencies require the separate prerequisite activation")
            result["precompile_guards"] = guards()
            require_guards(result["precompile_guards"])
            (output / "source-activated.json").write_text(json.dumps(active, indent=2) + "\n")
            run("phase-lifetime", [sys.executable, "-m", "unittest", "discover", "-s",
                                   "scripts/operations/tests", "-p", "test_phase_lifetime.py", "-v"], 15)
            # Own checkout mtimes only, after activation and before any compiler.
            for name in subprocess.check_output(["rg", "--files", "-g", "*.rs"], cwd=ROOT, text=True).splitlines():
                (ROOT / name).touch()
            run("rust-version", ["rustc", "--version"], 10)
            run("cargo-version", ["cargo", "--version"], 10)
            run("fmt", ["cargo", "fmt", "--all", "--", "--check"], 30)
            run("precompile", ["cargo", "test", "-p", "deepref-telemetry", "--lib", "--features", FEATURE, "--no-run", "--locked"])
            run("clippy", ["cargo", "clippy", "-p", "deepref-telemetry", "--all-targets", "--features", FEATURE, "--locked", "--", "-D", "warnings"])
            run("inventory", ["cargo", "nextest", "list", "-p", "deepref-telemetry", "--lib", "--features", FEATURE, "--locked", "-E", "test(" + CASE + ")"], 30)
            if CASE not in (output / "inventory.log").read_text():
                raise RuntimeError("exact capture case absent from inventory")
            result["pre_fixture_guards"] = guards()
            require_guards(result["pre_fixture_guards"])
            run("real-capture", ["cargo", "nextest", "run", "-p", "deepref-telemetry", "--lib", "--features", FEATURE,
                                 "--locked", "--retries", "0", "-E", "test(" + CASE + ")"], 65)
            cleanup = read_json_bounded(output / "fixture/cleanup.json")
            if (not cleanup["listener_independently_closed"] or not cleanup["server_joined"]
                    or cleanup["live_connections"] or cleanup["active_rpcs"] or cleanup["cleanup_errors"]
                    or cleanup["requests"] != cleanup["retained_requests"] or not 6 <= cleanup["requests"] <= 16):
                raise RuntimeError("actual gRPC cleanup incomplete")
            result["fixture_cleanup"] = cleanup
            result["actual_phases"] = phase_assertions(output / "fixture")
            result["exit"] = 0
        except BaseException as error:
            result.update(failure_type=type(error).__name__, failure=str(error))
        finally:
            try:
                final_inventory(output, nonce, active, result)
            finally:
                try:
                    if alias:
                        os.close(9)
                finally:
                    fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
    result["flock_released"] = True
    try:
        with FIXTURE_LOCK.open("rb") as probe:
            fcntl.flock(probe.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(probe.fileno(), fcntl.LOCK_UN)
        result["independent_flock_probe"] = "acquired-and-released"
    except OSError as error:
        result["independent_flock_probe"] = {"error": type(error).__name__, "errno": error.errno}
        result["cleanup_exit"] = 1
    result["finished_at"] = datetime.now(timezone.utc).isoformat()
    result["final_exit"] = 0 if result["exit"] == 0 and result["cleanup_exit"] == 0 else 1
    result["local_fixture_sdk_gRPC_capture"] = result["final_exit"] == 0
    if output is not None:
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return result["final_exit"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--pin-only", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.pin_only:
        with args.manifest.open("x") as output:
            json.dump(pins(), output, indent=2)
            output.write("\n")
        return 0
    if args.output is None or not args.output.is_absolute():
        parser.error("actual run requires new absolute output directory")
    try:
        os.fstat(9)
    except OSError as error:
        if error.errno != errno.EBADF:
            raise
    else:
        parser.error("runner owns its flock; do not inherit an outer descriptor9")
    def terminate(_signal, _frame):
        raise TimeoutError("owned capture runner terminated")
    signal.signal(signal.SIGTERM, terminate)
    return execute(args)


if __name__ == "__main__":
    raise SystemExit(main())
