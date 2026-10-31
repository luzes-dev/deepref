#!/usr/bin/env python3
"""One routed, nonblocking-flock local observability test batch; no services.

--pin-only writes source identities without acquiring resources. Actual execution
requires an explicit coordinator slot and this saved manifest. No Cargo, Docker,
external notification destination, arbitrary socket or cache purge is used.
"""

import argparse
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import time

from fault_proxy import FIXTURE_LOCK

ROOT = Path(__file__).resolve().parents[2]
OUTER_SECONDS = 90
MIN_DISK = 4 * 1024**3
MIN_MEMORY = 6 * 1024**3
SOURCE_FILES = (
    "scripts/operations/test_observability.py",
    "scripts/operations/alert_delivery.py", "scripts/operations/alert_protocol.py",
    "scripts/operations/metrics_catalog.py", "scripts/operations/fault_proxy.py",
    "scripts/operations/tests/test_alert_delivery_wire.py",
    "scripts/operations/tests/test_alert_protocol.py", "scripts/operations/tests/test_metrics_catalog.py",
    "scripts/operations/tests/test_dashboard_expiry.cjs",
    "operations/metrics/catalog.json", "operations/metrics/snapshot.example.json",
    "docs/operations-observability-runbook.md", "docs/metrics-and-alerts-runbook.md", "docs/README.md",
)


def identity():
    return {"head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, timeout=5).strip(),
            "tree": subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=ROOT, text=True, timeout=5).strip(),
            "status": subprocess.check_output(["git", "status", "--porcelain=v1"], cwd=ROOT, text=True, timeout=5),
            "files": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCE_FILES}}


def guards():
    fields = dict(line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines())
    return {"free_bytes": shutil.disk_usage(ROOT).free,
            "mem_available_bytes": int(fields["MemAvailable"].split()[0]) * 1024,
            "minimum_free_bytes": MIN_DISK, "minimum_mem_available_bytes": MIN_MEMORY}


def owned_closed(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return False
    except ConnectionRefusedError:
        return True


def phase_group_exists(group):
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    return True


def phase_group_inventory(phases):
    return [{"process_group": phase["process_group"],
             "closed": not phase_group_exists(phase["process_group"])}
            for phase in phases if "process_group" in phase]


def run_phase(name, command, *, output, environment, deadline, receipt):
    receipt.update(name=name, command=command, exit=None, process_exited=False)
    remaining = deadline - time.monotonic() - 5  # reserve cleanup/independent closure
    if remaining <= 0:
        raise TimeoutError("outer batch budget exhausted")
    started = time.monotonic()
    with (output / (name + ".log")).open("x") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=log,
                                   stderr=subprocess.STDOUT, pass_fds=(9,), start_new_session=True)
        receipt["process_group"] = process.pid
        try:
            code = process.wait(timeout=remaining)
            if phase_group_exists(process.pid):
                raise RuntimeError("phase descendants outlived their leader")
        except BaseException as error:
            receipt.update(failure_type=type(error).__name__, incomplete=True,
                           timed_out=isinstance(error, subprocess.TimeoutExpired))
            # Only this runner's newly created child process group is terminated.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
            # A leader may already have exited on TERM while its descendants
            # ignored it. Escalation belongs to the group, not leader state.
            if phase_group_exists(process.pid):
                receipt["forced_group_kill"] = True
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.wait(timeout=1)
            closed_deadline = time.monotonic() + 1
            while phase_group_exists(process.pid) and time.monotonic() < closed_deadline:
                time.sleep(0.01)
            if phase_group_exists(process.pid):
                raise RuntimeError("owned phase process group did not close") from error
            raise
        finally:
            receipt.update(seconds=time.monotonic() - started,
                           actual_child_exit=process.poll(), process_exited=process.poll() is not None,
                           process_group_closed=not phase_group_exists(process.pid))
    receipt["exit"] = code
    return receipt


def inventories(output):
    events = []
    for name in ("owned-ports.jsonl", "cli-delivery.jsonl"):
        path = output / name
        if path.exists():
            for line in path.read_text().splitlines():
                row = json.loads(line)
                port = row.get("port", row.get("owned_port"))
                if port is not None:
                    if type(port) is not int or not 1 <= port <= 65535:
                        raise ValueError("invalid owned port evidence")
                    events.append(row)
    if len(events) > 100:
        raise ValueError("unexpected owned port inventory")
    closed = []
    for port in sorted({row.get("port", row.get("owned_port")) for row in events}):
        closed.append({"port": port, "independently_closed": owned_closed(port)})
    return {"events": events, "ports": closed, "all_closed": all(row["independently_closed"] for row in closed)}


def execute(args):
    deadline = time.monotonic() + OUTER_SECONDS
    try:
        os.fstat(9)
    except OSError as error:
        if error.errno != errno.EBADF:
            raise
    else:
        raise ValueError("runner owns its lock; descriptor9 must not be inherited")
    expected = json.loads(args.manifest.read_text())
    if expected != identity():
        raise ValueError("source manifest drift before acquisition")
    result = {"phases": [], "exit": 1, "cleanup_exit": 1, "production_accepted": False,
              "actual_backend_ingestion": False, "human_notification": False,
              "python_executable": sys.executable, "python_version": sys.version,
              "outer_budget_seconds": OUTER_SECONDS}
    output = None
    with FIXTURE_LOCK.open("a+") as lock:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno in {errno.EACCES, errno.EAGAIN}:
                raise RuntimeError("fixture slot busy; not queued") from error
            raise
        alias = lock.fileno() != 9
        if alias:
            os.dup2(lock.fileno(), 9, inheritable=True)
        else:
            os.set_inheritable(9, True)
        try:
            result["initial_guards"] = guards()
            if result["initial_guards"]["free_bytes"] < MIN_DISK or result["initial_guards"]["mem_available_bytes"] < MIN_MEMORY:
                raise RuntimeError("unchanged fixture resource guard refused")
            if identity() != expected:
                raise ValueError("source drift at acquisition")
            args.output.mkdir(mode=0o700)
            output = args.output
            (output / "source-before.json").write_text(json.dumps(expected, indent=2) + "\n")
            for name in ("tmp", "formatter-tmp", "node-compile-cache"):
                (output / name).mkdir(mode=0o700)
            (output / "owned-ports.jsonl").touch()
            environment = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1", "NODE_DISABLE_COMPILE_CACHE": "1",
                           "TMPDIR": str(output / "tmp"),
                           "DEEPREF_ALERT_PORT_MANIFEST": str(output / "owned-ports.jsonl")}
            # Ignore counterfactual-only overrides inherited from a prior shell.
            for key in ("DEEPREF_DELIVERY_SOURCE", "DEEPREF_CATALOG_SOURCE", "DEEPREF_EXPIRY_COUNTERFACTUAL"):
                environment.pop(key, None)
            commands = [
                ("node-version", ["node", "--version"]),
                ("pnpm-version", ["pnpm", "--version"]),
                ("catalog", [sys.executable, "-m", "unittest", "discover", "-s", "scripts/operations/tests", "-p", "test_metrics_catalog.py", "-v"]),
                ("protocol", [sys.executable, "-m", "unittest", "discover", "-s", "scripts/operations/tests", "-p", "test_alert_protocol.py", "-v"]),
                ("wire", [sys.executable, "-m", "unittest", "discover", "-s", "scripts/operations/tests", "-p", "test_alert_delivery_wire.py", "-v"]),
                ("cli-delivery", [sys.executable, "scripts/operations/alert_delivery.py", "--catalog", "operations/metrics/catalog.json", "--repository", str(ROOT), "--rule", "queue-age", "--output", str(output / "cli-delivery.jsonl")]),
                ("render", [sys.executable, "scripts/operations/metrics_catalog.py", "--catalog", "operations/metrics/catalog.json", "--repository", str(ROOT), "--output", str(output / "rendered")]),
                ("generated-script", ["node", "--test", "scripts/operations/tests/test_dashboard_expiry.cjs"]),
                ("docs", ["bash", "scripts/check-docs.sh"]),
                ("format", ["pnpm", "--filter", "@deepref/web", "exec", "prettier", "--check",
                            *[str(ROOT / name) for name in SOURCE_FILES if name.endswith((".md", ".json", ".cjs"))]]),
            ]
            for name, command in commands:
                phase_env = {**environment, "DEEPREF_DASHBOARD_HTML": str(output / "rendered/dashboard.html")}
                if name == "format":
                    phase_env.pop("NODE_DISABLE_COMPILE_CACHE", None)
                    phase_env.update(TMPDIR=str(output / "formatter-tmp"), NODE_COMPILE_CACHE=str(output / "node-compile-cache"))
                phase = {}
                result["phases"].append(phase)
                run_phase(name, command, output=output, environment=phase_env, deadline=deadline, receipt=phase)
                if phase["exit"] != 0:
                    raise RuntimeError("phase failed: " + name)
            for name, count in (("catalog", 19), ("protocol", 10), ("wire", 9)):
                raw = (output / (name + ".log")).read_text()
                if f"Ran {count} tests" not in raw or "\nOK\n" not in raw or "skipped=" in raw:
                    raise RuntimeError("test count/selection mismatch: " + name)
            script_log = (output / "generated-script.log").read_text()
            if "pass 7" not in script_log or "fail 0" not in script_log or "skipped 0" not in script_log:
                raise RuntimeError("generated-script selection mismatch")
            receipt = json.loads((output / "cli-delivery.jsonl").read_text().splitlines()[-1])
            if not receipt.get("local_fixture_delivery") or not receipt.get("owned_listener_closed") or receipt.get("pending_tasks") != 0 or any(receipt.get(key) is not False for key in ("production_accepted", "human_notification_verified", "backend_ingestion_verified")):
                raise RuntimeError("CLI delivery evidence incomplete")
            result["exit"] = 0
        except BaseException as error:
            result["failure_type"] = type(error).__name__
            result["failure"] = str(error)
        finally:
            try:
                result["final_guards"] = guards()
                result["source_after"] = identity()
                if result["source_after"] != expected:
                    raise ValueError("source drift during batch")
                result["phase_groups"] = phase_group_inventory(result["phases"])
                if not all(row["closed"] for row in result["phase_groups"]):
                    raise RuntimeError("owned phase process group remains")
                if output is not None:
                    result["owned_inventory"] = inventories(output)
                    if result["exit"] == 0:
                        events = result["owned_inventory"]["events"]
                        starts = [row for row in events if row.get("phase") == "listener_started"]
                        closed = [row for row in events if row.get("phase") == "closed"]
                        if len(starts) != 15 or len(closed) != 14 or any(row["pending_tasks"] != 0 or row["failed_clients"] for row in closed):
                            raise RuntimeError("owned listener/task inventory incomplete")
                    result["owned_tmp_entries"] = [str(p.relative_to(output)) for name in ("tmp", "formatter-tmp") for p in (output / name).rglob("*")]
                    if result["owned_tmp_entries"] or not result["owned_inventory"]["all_closed"]:
                        raise RuntimeError("owned listener or temporary fixture cleanup failed")
                    result["retained_derived_cache"] = [{"path": str(p.relative_to(output)), "bytes": p.stat().st_size,
                                                         "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
                                                        for p in (output / "node-compile-cache").rglob("*") if p.is_file()]
                result["cleanup_exit"] = 0
            except BaseException as error:
                result["cleanup_failure"] = str(error)
            finally:
                if alias:
                    os.close(9)
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
    # Successful acquisition by an independent opener confirms actual release.
    try:
        with FIXTURE_LOCK.open("rb") as probe:
            fcntl.flock(probe.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(probe.fileno(), fcntl.LOCK_UN)
        result["flock_released"] = True
    except OSError as error:
        result.update(flock_released=False, release_probe_errno=error.errno)
    result["final_exit"] = 0 if result["exit"] == 0 and result["cleanup_exit"] == 0 and result["flock_released"] else 1
    if output is not None:
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return result["final_exit"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True, help="new pin-only output or saved execution pins")
    parser.add_argument("--pin-only", action="store_true")
    parser.add_argument("--output", type=Path, help="new evidence directory, required for actual run")
    args = parser.parse_args()
    if args.pin_only:
        with args.manifest.open("x") as output:
            json.dump(identity(), output, indent=2)
            output.write("\n")
        return 0
    if args.output is None or not args.output.is_absolute():
        parser.error("actual batch requires a new absolute --output directory")
    def terminate(_signal, _frame):
        raise TimeoutError("outer runner termination; cleaning owned resources")

    signal.signal(signal.SIGTERM, terminate)
    return execute(args)


if __name__ == "__main__":
    raise SystemExit(main())
