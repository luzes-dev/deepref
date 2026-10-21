#!/usr/bin/env python3
"""Run DeepRef CI probes and turn their results into one compact report.

The runner is deliberately dependency-free: it uses only the Python standard
library, does not import the Rust or Node toolchains, and can execute even when
the Cargo workspace (or ``xtask``) does not compile.

Terminology:

* probe -- one independently useful diagnostic command from diagnostics.toml;
* gate  -- the final lane decision, evaluated after all reachable probes;
* blocked -- a probe that could not produce meaningful output because a
  prerequisite failed.  Blocked is not another failure.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping, Sequence

try:
    import tomllib
except ModuleNotFoundError as exc:  # pragma: no cover - Python < 3.11
    raise SystemExit("diagnose.py requires Python 3.11 or newer (tomllib)") from exc

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = REPO_ROOT / "scripts" / "ci" / "diagnostics.toml"
DEFAULT_OUTPUT = REPO_ROOT / "target" / "ci-diagnostics"

STATUS_PASSED = "passed"
STATUS_FAILED = "failed"
STATUS_BLOCKED = "blocked"
ALL_STATUSES = {STATUS_PASSED, STATUS_FAILED, STATUS_BLOCKED}
# Merge duplicate evidence conservatively: never hide a failure, prefer a real
# pass over a blocked gate observation, and keep blocked only when it is the
# strongest available evidence.
STATUS_RANK = {STATUS_BLOCKED: 0, STATUS_PASSED: 1, STATUS_FAILED: 2}

CHECK_ID_RE = re.compile(r"^[a-z0-9][a-z0-9_.-]*$")
ANSI_RE = re.compile(
    r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\)|[@-_])"
)
TIMESTAMP_PREFIX_RE = re.compile(
    r"^\s*(?:\[?\d{4}-\d{2}-\d{2}[T ][0-9:.+-]+Z?\]?)\s*"
)
NOISE_LINE_RE = re.compile(
    r"^\s*(?:"
    r"Compiling\b|Checking\b|Fresh\b|Downloading\b|Downloaded\b|"
    r"Blocking waiting for file lock\b|Finished `|"
    r"\[\s*\d+/\d+\]"
    r")"
)
LOCATION_RE = re.compile(
    r"(?:[A-Za-z0-9_.@+-]+/)+[A-Za-z0-9_.@+-]+\.[A-Za-z0-9]+"
    r":\d+(?::\d+)?"
)
SEVERITY_RE = re.compile(r"\b(?:error|failed|failure|panic|warning)\b", re.IGNORECASE)


class DiagnosticError(Exception):
    """A usage, manifest, or infrastructure error in the diagnostic runner."""


@dataclass(frozen=True)
class Check:
    id: str
    group: str
    command: tuple[str, ...]
    depends_on: tuple[str, ...] = ()
    env: Mapping[str, str] | None = None
    description: str | None = None


@dataclass
class Manifest:
    checks: Mapping[str, Check]
    order: tuple[str, ...]
    excerpt_lines: int = 60
    excerpt_chars: int = 6000

    def groups(self) -> list[str]:
        seen: list[str] = []
        for check_id in self.order:
            group = self.checks[check_id].group
            if group not in seen:
                seen.append(group)
        return seen

    def checks_in_groups(self, groups: Sequence[str]) -> list[str]:
        wanted = set(groups)
        return [
            check_id
            for check_id in self.order
            if self.checks[check_id].group in wanted
        ]


def _resolve_path(raw: str | os.PathLike[str], base: Path = REPO_ROOT) -> Path:
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = base / path
    return path.resolve()


def load_manifest(path: Path) -> Manifest:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise DiagnosticError(f"cannot read manifest {path}: {exc}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise DiagnosticError(f"malformed manifest {path}: {exc}") from exc

    raw_checks = data.get("check", [])
    if not isinstance(raw_checks, list):
        raise DiagnosticError(f"malformed manifest {path}: 'check' must be an array")

    checks: dict[str, Check] = {}
    order: list[str] = []
    for index, raw in enumerate(raw_checks, start=1):
        if not isinstance(raw, dict):
            raise DiagnosticError(
                f"malformed manifest {path}: check #{index} must be a table"
            )

        check_id = raw.get("id")
        if not isinstance(check_id, str) or not CHECK_ID_RE.match(check_id):
            raise DiagnosticError(
                f"malformed manifest {path}: check #{index} has invalid id {check_id!r}"
            )
        if check_id in checks:
            raise DiagnosticError(
                f"malformed manifest {path}: duplicate check id {check_id!r}"
            )

        group = raw.get("group")
        if not isinstance(group, str) or not CHECK_ID_RE.match(group):
            raise DiagnosticError(
                f"malformed manifest {path}: check {check_id!r} has invalid group {group!r}"
            )

        command = raw.get("command")
        if (
            not isinstance(command, list)
            or not command
            or not all(isinstance(argument, str) and argument for argument in command)
        ):
            raise DiagnosticError(
                f"malformed manifest {path}: check {check_id!r} must define a "
                "non-empty command argv array"
            )

        depends_on = raw.get("depends_on", [])
        if not isinstance(depends_on, list) or not all(
            isinstance(dependency, str) for dependency in depends_on
        ):
            raise DiagnosticError(
                f"malformed manifest {path}: check {check_id!r} has invalid depends_on"
            )

        env = raw.get("env", {})
        if not isinstance(env, dict) or not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in env.items()
        ):
            raise DiagnosticError(
                f"malformed manifest {path}: check {check_id!r} has invalid env"
            )

        description = raw.get("description")
        if description is not None and not isinstance(description, str):
            raise DiagnosticError(
                f"malformed manifest {path}: check {check_id!r} has invalid description"
            )

        checks[check_id] = Check(
            id=check_id,
            group=group,
            command=tuple(command),
            depends_on=tuple(depends_on),
            env=dict(env),
            description=description,
        )
        order.append(check_id)

    for check in checks.values():
        for dependency in check.depends_on:
            if dependency not in checks:
                raise DiagnosticError(
                    f"malformed manifest {path}: check {check.id!r} depends on "
                    f"unknown check {dependency!r}"
                )

    _assert_acyclic(checks)

    report_settings = data.get("report", {})
    if not isinstance(report_settings, dict):
        raise DiagnosticError(
            f"malformed manifest {path}: 'report' must be a table"
        )
    excerpt_lines = report_settings.get("excerpt_lines", 60)
    excerpt_chars = report_settings.get("excerpt_chars", 6000)
    if not isinstance(excerpt_lines, int) or excerpt_lines <= 0:
        raise DiagnosticError(
            f"malformed manifest {path}: report.excerpt_lines must be a positive integer"
        )
    if not isinstance(excerpt_chars, int) or excerpt_chars <= 0:
        raise DiagnosticError(
            f"malformed manifest {path}: report.excerpt_chars must be a positive integer"
        )

    return Manifest(
        checks=checks,
        order=tuple(order),
        excerpt_lines=excerpt_lines,
        excerpt_chars=excerpt_chars,
    )


def _assert_acyclic(checks: Mapping[str, Check]) -> None:
    visiting: set[str] = set()
    done: set[str] = set()

    def visit(check_id: str, stack: list[str]) -> None:
        if check_id in done:
            return
        if check_id in visiting:
            cycle_start = stack.index(check_id)
            cycle = " -> ".join(stack[cycle_start:] + [check_id])
            raise DiagnosticError(f"malformed manifest: dependency cycle {cycle}")
        visiting.add(check_id)
        stack.append(check_id)
        for dependency in checks[check_id].depends_on:
            visit(dependency, stack)
        stack.pop()
        visiting.remove(check_id)
        done.add(check_id)

    for check_id in checks:
        visit(check_id, [])


def _result_path(output_dir: Path, check_id: str) -> Path:
    return output_dir / "checks" / f"{check_id}.json"


def _log_path(output_dir: Path, check_id: str) -> Path:
    return output_dir / "logs" / f"{check_id}.log"


def _write_json_atomic(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


def _write_log(output_dir: Path, check_id: str, content: str) -> str:
    path = _log_path(output_dir, check_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return f"logs/{check_id}.log"


def _save_result(output_dir: Path, result: Mapping[str, object]) -> None:
    _write_json_atomic(_result_path(output_dir, str(result["id"])), result)


def load_results(output_dir: Path) -> dict[str, dict[str, object]]:
    checks_dir = output_dir / "checks"
    if not checks_dir.is_dir():
        return {}
    results: dict[str, dict[str, object]] = {}
    for path in sorted(checks_dir.glob("*.json")):
        result = _load_json(path)
        _validate_result(result, path)
        results[str(result["id"])] = result
    return results


def _load_json(path: Path) -> dict[str, object]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise DiagnosticError(f"cannot read {path}: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise DiagnosticError(f"malformed JSON in {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise DiagnosticError(f"expected a JSON object in {path}")
    return data


def _validate_result(result: Mapping[str, object], source: Path) -> None:
    check_id = result.get("id")
    if not isinstance(check_id, str) or not CHECK_ID_RE.match(check_id):
        raise DiagnosticError(f"{source}: missing or invalid check id")
    group = result.get("group")
    if not isinstance(group, str) or not CHECK_ID_RE.match(group):
        raise DiagnosticError(f"{source}: missing or invalid group for {check_id}")
    status = result.get("status")
    if status not in ALL_STATUSES:
        raise DiagnosticError(f"{source}: invalid status {status!r} for {check_id}")


def _dependency_failures(
    check: Check, results: Mapping[str, Mapping[str, object]]
) -> list[str]:
    failures: list[str] = []
    for dependency in check.depends_on:
        dependency_result = results.get(dependency)
        if dependency_result is None:
            failures.append(f"{dependency} (not run)")
        elif dependency_result.get("status") != STATUS_PASSED:
            failures.append(f"{dependency} ({dependency_result.get('status')})")
    return failures


def _blocked_result(
    check: Check, reasons: Sequence[str], output_dir: Path
) -> dict[str, object]:
    reason = "; ".join(reasons) if reasons else "probe did not run"
    log = _write_log(output_dir, check.id, f"blocked: {reason}\n")
    return {
        "id": check.id,
        "group": check.group,
        "status": STATUS_BLOCKED,
        "exit_code": None,
        "duration_ms": 0,
        "log": log,
        "depends_on": list(check.depends_on),
        "blocked_by": list(check.depends_on) if check.depends_on else [],
        "error": f"blocked: {reason}",
        "command": list(check.command),
    }


def _execute_check(check: Check, output_dir: Path) -> dict[str, object]:
    log = _log_path(output_dir, check.id)
    log.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    exit_code: int | None = None
    error: str | None = None
    status = STATUS_FAILED

    environment = os.environ.copy()
    environment.update(check.env or {})
    # Commands always run at REPO_ROOT; keep shell-visible PWD in sync so
    # probes such as Semgrep can mount the checkout deterministically.
    environment["PWD"] = str(REPO_ROOT)

    try:
        with log.open("wb") as stream:
            try:
                completed = subprocess.run(
                    list(check.command),
                    cwd=REPO_ROOT,
                    env=environment,
                    stdout=stream,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
                exit_code = completed.returncode
                status = STATUS_PASSED if exit_code == 0 else STATUS_FAILED
            except FileNotFoundError as exc:
                message = (
                    f"diagnostic infrastructure error: executable not found: "
                    f"{check.command[0]}\n{exc}\n"
                )
                stream.write(message.encode("utf-8", errors="replace"))
                exit_code = 127
                error = f"executable not found: {check.command[0]}"
                status = STATUS_FAILED
            except PermissionError as exc:
                message = (
                    f"diagnostic infrastructure error: cannot execute "
                    f"{check.command[0]}: {exc}\n"
                )
                stream.write(message.encode("utf-8", errors="replace"))
                exit_code = 126
                error = f"cannot execute: {check.command[0]}"
                status = STATUS_FAILED
            except OSError as exc:
                message = f"diagnostic infrastructure error: {exc}\n"
                stream.write(message.encode("utf-8", errors="replace"))
                exit_code = 125
                error = str(exc)
                status = STATUS_FAILED
    except OSError as exc:
        # The log itself could not be opened.  Keep the public model intact and
        # surface the infrastructure failure rather than skipping the probe.
        error = f"cannot write probe log {log}: {exc}"
        status = STATUS_FAILED
        exit_code = 125

    duration_ms = int(round((time.monotonic() - started) * 1000))
    result: dict[str, object] = {
        "id": check.id,
        "group": check.group,
        "status": status,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
        "log": f"logs/{check.id}.log" if log.is_file() else None,
        "depends_on": list(check.depends_on),
        "command": list(check.command),
    }
    if error is not None:
        result["error"] = error
    return result


def _run_check(
    check: Check,
    results: Mapping[str, Mapping[str, object]],
    output_dir: Path,
) -> dict[str, object]:
    failures = _dependency_failures(check, results)
    if failures:
        result = _blocked_result(
            check,
            [f"dependency did not pass: {failure}" for failure in failures],
            output_dir,
        )
    else:
        result = _execute_check(check, output_dir)
    _save_result(output_dir, result)
    _print_result(result)
    return result


def _topological_order(selected: Iterable[str], manifest: Manifest) -> list[str]:
    wanted = set(selected)
    ordered: list[str] = []
    visited: set[str] = set()

    def visit(check_id: str) -> None:
        if check_id in visited:
            return
        visited.add(check_id)
        for dependency in manifest.checks[check_id].depends_on:
            if dependency in wanted:
                visit(dependency)
        ordered.append(check_id)

    for check_id in manifest.order:
        if check_id in wanted:
            visit(check_id)
    return ordered


def _status_label(status: object) -> str:
    return {
        STATUS_PASSED: "PASS",
        STATUS_FAILED: "FAIL",
        STATUS_BLOCKED: "BLOCKED",
    }.get(str(status), str(status).upper())


def _print_result(result: Mapping[str, object]) -> None:
    duration = result.get("duration_ms")
    suffix = f" ({duration} ms)" if isinstance(duration, int) else ""
    print(f"{_status_label(result.get('status')):<8} {result.get('id')}{suffix}")


def _print_table(results: Sequence[Mapping[str, object]]) -> None:
    print()
    print(f"{'Probe':<40} {'Status':<8} Details")
    print("-" * 78)
    for result in results:
        details = result.get("error") or result.get("log") or ""
        print(
            f"{str(result.get('id')):<40} "
            f"{_status_label(result.get('status')):<8} {details}"
        )


def _clean_log_text(raw: str) -> str:
    text = ANSI_RE.sub("", raw)
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    cleaned: list[str] = []
    blank = False
    for line in text.splitlines():
        line = TIMESTAMP_PREFIX_RE.sub("", line.rstrip())
        if NOISE_LINE_RE.match(line):
            continue
        if not line:
            if blank:
                continue
            blank = True
        else:
            blank = False
        cleaned.append(line)
    return "\n".join(cleaned).strip("\n")


def _bounded_excerpt(text: str, max_lines: int, max_chars: int) -> str:
    if not text:
        return "(no diagnostic output captured)"
    lines = text.splitlines()
    if len(lines) > max_lines:
        head = min(max(1, max_lines // 3), max_lines - 1)
        tail = max_lines - head - 1
        omitted = len(lines) - head - tail
        lines = (
            lines[:head]
            + [f"... ({omitted} lines omitted; see the full log) ..."]
            + (lines[-tail:] if tail else [])
        )
    excerpt = "\n".join(lines)
    if len(excerpt) > max_chars:
        excerpt = excerpt[:max_chars].rstrip() + (
            "\n... (excerpt truncated; see the full log) ..."
        )
    return excerpt


def _read_log_text(output_dir: Path, result: Mapping[str, object]) -> str | None:
    log = result.get("log")
    if not isinstance(log, str) or not log:
        return None
    path = Path(log)
    if not path.is_absolute():
        path = output_dir / path
    try:
        return path.read_bytes().decode("utf-8", errors="replace")
    except OSError:
        return None


def _ordered_ids(ids: Iterable[str], manifest: Manifest) -> list[str]:
    wanted = set(ids)
    known = [check_id for check_id in manifest.order if check_id in wanted]
    return known + sorted(wanted - set(known))


def _groups_for(ids: Iterable[str], manifest: Manifest) -> list[str]:
    groups: list[str] = []
    for check_id in _ordered_ids(ids, manifest):
        check = manifest.checks.get(check_id)
        if check is None:
            continue
        if check.group not in groups:
            groups.append(check.group)
    return groups


def _public_result(result: Mapping[str, object]) -> dict[str, object]:
    public: dict[str, object] = {}
    for key in (
        "id",
        "group",
        "status",
        "exit_code",
        "duration_ms",
        "log",
        "depends_on",
        "blocked_by",
        "error",
        "command",
    ):
        if key in result:
            public[key] = result[key]
    return public


def write_report(
    results: Mapping[str, Mapping[str, object]],
    manifest: Manifest,
    output_dir: Path,
    groups: Sequence[str] | None = None,
    probe_ids: Iterable[str] | None = None,
) -> dict[str, object]:
    wanted_ids = (
        list(probe_ids)
        if probe_ids is not None
        else [check_id for check_id in results]
    )
    ordered = [
        check_id
        for check_id in _ordered_ids(wanted_ids, manifest)
        if check_id in results
    ]
    probes = [_public_result(results[check_id]) for check_id in ordered]

    if groups is None:
        report_groups = _groups_for(ordered, manifest)
        for probe in probes:
            group = probe.get("group")
            if isinstance(group, str) and group not in report_groups:
                report_groups.append(group)
    else:
        report_groups = list(groups)

    summary = {
        "total": len(probes),
        "passed": sum(1 for probe in probes if probe.get("status") == STATUS_PASSED),
        "failed": sum(1 for probe in probes if probe.get("status") == STATUS_FAILED),
        "blocked": sum(1 for probe in probes if probe.get("status") == STATUS_BLOCKED),
    }
    report: dict[str, object] = {
        "schema_version": 1,
        "groups": report_groups,
        "summary": summary,
        "probes": probes,
    }

    output_dir.mkdir(parents=True, exist_ok=True)
    _write_json_atomic(output_dir / "report.json", report)
    markdown = _render_markdown(report, manifest, output_dir)
    (output_dir / "report.md").write_text(markdown, encoding="utf-8")
    return report


def _render_markdown(
    report: Mapping[str, object], manifest: Manifest, output_dir: Path
) -> str:
    probes = report.get("probes", [])
    summary = report.get("summary", {})
    if not isinstance(probes, list):
        probes = []
    if not isinstance(summary, dict):
        summary = {}

    lines = [
        "# CI diagnostics",
        "",
        "| Probe | Status | Duration | Full log |",
        "| --- | --- | --- | --- |",
    ]
    for probe in probes:
        if not isinstance(probe, dict):
            continue
        duration = probe.get("duration_ms")
        duration_text = (
            f"{duration / 1000:.1f}s"
            if isinstance(duration, int) and duration >= 1000
            else (f"{duration}ms" if isinstance(duration, int) else "-")
        )
        log = probe.get("log")
        log_text = f"`{log}`" if isinstance(log, str) and log else "-"
        lines.append(
            f"| `{probe.get('id')}` | **{_status_label(probe.get('status'))}** | "
            f"{duration_text} | {log_text} |"
        )

    lines.extend(
        [
            "",
            (
                f"Summary: {summary.get('passed', 0)} passed, "
                f"{summary.get('failed', 0)} failed, "
                f"{summary.get('blocked', 0)} blocked "
                f"({summary.get('total', 0)} probes)."
            ),
        ]
    )

    failed = [
        probe
        for probe in probes
        if isinstance(probe, dict) and probe.get("status") == STATUS_FAILED
    ]
    blocked = [
        probe
        for probe in probes
        if isinstance(probe, dict) and probe.get("status") == STATUS_BLOCKED
    ]

    if failed:
        lines.extend(["", "## Failed probes"])
    failed_lines: dict[str, list[str]] = {}
    for probe in failed:
        assert isinstance(probe, dict)
        probe_id = str(probe.get("id"))
        log = probe.get("log")
        lines.extend(
            [
                "",
                f"### `{probe_id}`",
                f"Full log: `{log}`" if isinstance(log, str) and log else "Full log: not captured",
            ]
        )
        if probe.get("error"):
            lines.append(f"Error: {probe['error']}")
        raw = _read_log_text(output_dir, probe)
        if raw is None:
            lines.append("The full log is missing from the diagnostic directory.")
            failed_lines[probe_id] = []
            continue
        cleaned = _clean_log_text(raw)
        failed_lines[probe_id] = [
            line for line in cleaned.splitlines() if line.strip()
        ]
        excerpt = _bounded_excerpt(
            cleaned, manifest.excerpt_lines, manifest.excerpt_chars
        )
        lines.extend(["", "~~~~text", excerpt, "~~~~"])

    if blocked:
        lines.extend(["", "## Blocked probes"])
        for probe in blocked:
            assert isinstance(probe, dict)
            error = probe.get("error") or "blocked"
            log = probe.get("log")
            suffix = f" Full log: `{log}`" if isinstance(log, str) and log else ""
            lines.append(f"- `{probe.get('id')}`: {error}.{suffix}")

    repeated = _repeated_diagnostics(failed_lines)
    if repeated:
        lines.extend(["", "## Repeated diagnostics"])
        for key, probe_ids in repeated:
            display = key if len(key) <= 300 else key[:297] + "..."
            lines.append(
                f"- `{display}` -- seen in "
                + ", ".join(f"`{probe_id}`" for probe_id in probe_ids)
            )

    lines.append("")
    return "\n".join(lines)


def _repeated_diagnostics(
    failed_lines: Mapping[str, Sequence[str]],
) -> list[tuple[str, list[str]]]:
    """Return identical location-bearing diagnostics seen in multiple probes.

    Only exact normalized lines qualify.  This is intentionally conservative:
    anything that is not provably identical stays in its original probe excerpt.
    """

    seen: dict[str, list[str]] = {}
    for probe_id, lines in failed_lines.items():
        local: set[str] = set()
        for line in lines:
            normalized = re.sub(r"\s+", " ", line.strip())
            if not LOCATION_RE.search(normalized):
                continue
            if not SEVERITY_RE.search(normalized):
                continue
            local.add(normalized)
        for key in local:
            seen.setdefault(key, []).append(probe_id)
    return sorted(
        (key, sorted(probe_ids))
        for key, probe_ids in seen.items()
        if len(probe_ids) > 1
    )


def _append_step_summary(markdown: str) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    try:
        with Path(summary_path).open("a", encoding="utf-8") as stream:
            stream.write(markdown.rstrip() + "\n")
    except OSError as exc:
        print(
            f"diagnose: warning: cannot append to GITHUB_STEP_SUMMARY: {exc}",
            file=sys.stderr,
        )


def _validate_groups(manifest: Manifest, groups: Sequence[str]) -> list[str]:
    known = set(manifest.groups())
    unknown = [group for group in groups if group not in known]
    if unknown:
        raise DiagnosticError(
            "unknown diagnostic group(s): "
            + ", ".join(unknown)
            + "; known groups: "
            + ", ".join(sorted(known))
        )
    return list(groups)


def command_run_check(manifest: Manifest, output_dir: Path, check_id: str) -> int:
    check = manifest.checks.get(check_id)
    if check is None:
        raise DiagnosticError(f"unknown check id {check_id!r}")
    results = load_results(output_dir)
    result = _run_check(check, results, output_dir)
    # Blocked means the probe was not meaningful, not that it produced a new
    # failure.  The lane gate is responsible for turning blocked into red.
    return (
        0
        if result.get("status") in {STATUS_PASSED, STATUS_BLOCKED}
        else 1
    )


def command_run(
    manifest: Manifest, output_dir: Path, requested_groups: Sequence[str]
) -> int:
    groups = (
        _validate_groups(manifest, requested_groups)
        if requested_groups
        else manifest.groups()
    )
    if not groups:
        raise DiagnosticError("the diagnostic manifest defines no checks")
    selected = manifest.checks_in_groups(groups)
    if not selected:
        raise DiagnosticError("the selected groups define no checks")

    # Results from a previous run in the same working directory must not make a
    # probe look green when this run never reached it.
    for check_id in selected:
        _result_path(output_dir, check_id).unlink(missing_ok=True)
        _log_path(output_dir, check_id).unlink(missing_ok=True)

    results = load_results(output_dir)
    for check_id in _topological_order(selected, manifest):
        result = _run_check(manifest.checks[check_id], results, output_dir)
        results[check_id] = result

    report = write_report(
        results,
        manifest,
        output_dir,
        groups=groups,
        probe_ids=selected,
    )
    probes = report.get("probes", [])
    if isinstance(probes, list):
        _print_table([probe for probe in probes if isinstance(probe, dict)])
    return 0 if all(
        isinstance(probe, dict) and probe.get("status") == STATUS_PASSED
        for probe in probes
    ) else 1


def command_gate(
    manifest: Manifest, output_dir: Path, requested_groups: Sequence[str]
) -> int:
    groups = (
        _validate_groups(manifest, requested_groups)
        if requested_groups
        else manifest.groups()
    )
    if not groups:
        raise DiagnosticError("the diagnostic manifest defines no checks")
    expected = manifest.checks_in_groups(groups)
    if not expected:
        raise DiagnosticError("the selected groups define no checks")

    results = load_results(output_dir)
    for check_id in expected:
        if check_id in results:
            continue
        check = manifest.checks[check_id]
        failures = _dependency_failures(check, results)
        if failures:
            reasons = [
                f"not reached because dependency did not pass: {failure}"
                for failure in failures
            ]
        else:
            reasons = ["not reached (probe step did not run)"]
        result = _blocked_result(check, reasons, output_dir)
        _save_result(output_dir, result)
        results[check_id] = result

    report = write_report(
        results,
        manifest,
        output_dir,
        groups=groups,
        probe_ids=expected,
    )
    markdown = (output_dir / "report.md").read_text(encoding="utf-8")
    _append_step_summary(markdown)
    probes = report.get("probes", [])
    if isinstance(probes, list):
        _print_table([probe for probe in probes if isinstance(probe, dict)])
    return 0 if all(
        isinstance(probe, dict) and probe.get("status") == STATUS_PASSED
        for probe in probes
    ) else 1


def _select_rerun(
    previous: Mapping[str, Mapping[str, object]], manifest: Manifest
) -> set[str]:
    selected = {
        check_id
        for check_id, result in previous.items()
        if result.get("status") == STATUS_FAILED
    }
    changed = True
    while changed:
        changed = False
        for check_id, result in previous.items():
            if check_id in selected or result.get("status") != STATUS_BLOCKED:
                continue
            check = manifest.checks.get(check_id)
            if check is None:
                continue
            reachable = all(
                dependency in previous
                and (
                    previous[dependency].get("status") == STATUS_PASSED
                    or dependency in selected
                )
                for dependency in check.depends_on
            )
            if reachable:
                selected.add(check_id)
                changed = True
    return selected


def _materialize_log(
    result: Mapping[str, object], source_base: Path, output_dir: Path
) -> dict[str, object]:
    materialized = dict(result)
    log = materialized.get("log")
    if not isinstance(log, str) or not log:
        return materialized
    source = Path(log)
    if not source.is_absolute():
        source = source_base / source
    target = _log_path(output_dir, str(materialized["id"]))
    try:
        same_file = source.resolve() == target.resolve()
    except OSError:
        same_file = False
    if same_file:
        materialized["log"] = f"logs/{materialized['id']}.log"
        return materialized
    if source.is_file():
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        materialized["log"] = f"logs/{materialized['id']}.log"
    return materialized


def _load_report(path: Path) -> dict[str, object]:
    if path.is_dir():
        path = path / "report.json"
    data = _load_json(path)
    probes = data.get("probes")
    if not isinstance(probes, list):
        raise DiagnosticError(f"{path}: expected a report with a 'probes' array")
    for probe in probes:
        if not isinstance(probe, dict):
            raise DiagnosticError(f"{path}: report contains a non-object probe")
        _validate_result(probe, path)
    return data


def command_rerun_failed(
    manifest: Manifest, output_dir: Path, report_path: Path
) -> int:
    report_file = (
        report_path / "report.json" if report_path.is_dir() else report_path
    )
    report = _load_report(report_file)
    source_base = report_file.parent
    previous: dict[str, dict[str, object]] = {}
    for probe in report["probes"]:  # type: ignore[index]
        assert isinstance(probe, dict)
        materialized = _materialize_log(probe, source_base, output_dir)
        previous[str(materialized["id"])] = materialized

    unknown = sorted(set(previous) - set(manifest.checks))
    if unknown:
        raise DiagnosticError(
            "report contains checks that are not in the manifest: "
            + ", ".join(unknown)
        )

    selected = _select_rerun(previous, manifest)
    if selected:
        print("Rerunning failed and newly reachable probes: " + ", ".join(
            check_id for check_id in manifest.order if check_id in selected
        ))
    results = previous
    for check_id in selected:
        _result_path(output_dir, check_id).unlink(missing_ok=True)
        _log_path(output_dir, check_id).unlink(missing_ok=True)
        results.pop(check_id, None)

    for check_id in _topological_order(selected, manifest):
        result = _run_check(manifest.checks[check_id], results, output_dir)
        results[check_id] = result

    groups = _groups_for(results, manifest)
    report = write_report(
        results,
        manifest,
        output_dir,
        groups=groups,
        probe_ids=list(results),
    )
    probes = report.get("probes", [])
    if isinstance(probes, list):
        _print_table([probe for probe in probes if isinstance(probe, dict)])
    return 0 if all(
        isinstance(probe, dict) and probe.get("status") == STATUS_PASSED
        for probe in probes
    ) else 1


def _is_relative_to(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def _load_result_file(
    path: Path, output_dir: Path
) -> list[tuple[dict[str, object], Path]]:
    data = _load_json(path)
    if isinstance(data.get("probes"), list):
        base = path.parent
        loaded: list[tuple[dict[str, object], Path]] = []
        for probe in data["probes"]:  # type: ignore[index]
            if not isinstance(probe, dict):
                raise DiagnosticError(f"{path}: report contains a non-object probe")
            _validate_result(probe, path)
            loaded.append((dict(probe), base))
        return loaded
    if "id" in data and "status" in data:
        _validate_result(data, path)
        base = path.parent.parent if path.parent.name == "checks" else path.parent
        return [(dict(data), base)]
    raise DiagnosticError(
        f"{path}: expected a diagnostic check result or a report with 'probes'"
    )


def _load_results_from_path(
    path: Path, output_dir: Path
) -> list[tuple[dict[str, object], Path]]:
    if not path.exists():
        raise DiagnosticError(f"diagnostic input does not exist: {path}")
    if path.is_file():
        return _load_result_file(path, output_dir)
    if not path.is_dir():
        raise DiagnosticError(f"diagnostic input is not a file or directory: {path}")

    loaded: list[tuple[dict[str, object], Path]] = []
    seen: set[Path] = set()
    for candidate in sorted(path.rglob("*.json")):
        if not candidate.is_file() or candidate in seen:
            continue
        if _is_relative_to(candidate, output_dir):
            continue
        if candidate.name != "report.json" and candidate.parent.name != "checks":
            continue
        seen.add(candidate)
        loaded.extend(_load_result_file(candidate, output_dir))
    return loaded


def _result_rank(
    result: Mapping[str, object], source_base: Path
) -> tuple[int, int, str]:
    status = str(result.get("status"))
    log = result.get("log")
    has_log = 0
    if isinstance(log, str) and log:
        candidate = Path(log)
        if not candidate.is_absolute():
            candidate = source_base / candidate
        has_log = 1 if candidate.is_file() else 0
    return (STATUS_RANK.get(status, 99), has_log, json.dumps(result, sort_keys=True))


def command_merge(
    manifest: Manifest, output_dir: Path, raw_paths: Sequence[str]
) -> int:
    if not raw_paths:
        raise DiagnosticError("merge requires at least one report or diagnostic directory")

    chosen: dict[str, tuple[dict[str, object], Path]] = {}
    for raw_path in raw_paths:
        path = _resolve_path(raw_path)
        for result, base in _load_results_from_path(path, output_dir):
            check_id = str(result["id"])
            current = chosen.get(check_id)
            if current is None or _result_rank(result, base) > _result_rank(
                current[0], current[1]
            ):
                chosen[check_id] = (result, base)

    results: dict[str, dict[str, object]] = {}
    for check_id in sorted(chosen):
        result, base = chosen[check_id]
        materialized = _materialize_log(result, base, output_dir)
        results[check_id] = materialized
        _save_result(output_dir, materialized)

    report = write_report(
        results,
        manifest,
        output_dir,
        groups=None,
        probe_ids=list(results),
    )
    probes = report.get("probes", [])
    if isinstance(probes, list):
        _print_table([probe for probe in probes if isinstance(probe, dict)])
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="diagnose.py",
        description=(
            "Run DeepRef CI probes, gate lanes, rerun only failed/reachable probes, "
            "or merge lane reports into one compact diagnostic report."
        ),
        epilog=(
            "Examples:\n"
            "  python3 scripts/ci/diagnose.py run web\n"
            "  python3 scripts/ci/diagnose.py run rust integration\n"
            "  python3 scripts/ci/diagnose.py run-check web.lint\n"
            "  python3 scripts/ci/diagnose.py gate web\n"
            "  python3 scripts/ci/diagnose.py rerun-failed target/ci-diagnostics/report.json\n"
            "  python3 scripts/ci/diagnose.py merge diagnostics-artifacts\n"
            "\n"
            "Global options must precede the subcommand. Results and full logs are "
            "written below --output (default: target/ci-diagnostics)."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--manifest",
        default=str(DEFAULT_MANIFEST),
        help="probe manifest (default: scripts/ci/diagnostics.toml)",
    )
    parser.add_argument(
        "--output",
        default=str(DEFAULT_OUTPUT),
        help="diagnostic working directory (default: target/ci-diagnostics)",
    )

    subparsers = parser.add_subparsers(dest="command", required=True)

    run_check = subparsers.add_parser(
        "run-check", help="run one probe, honouring its dependencies"
    )
    run_check.add_argument("check_id")

    run = subparsers.add_parser(
        "run", help="run every probe in one or more groups (default: all groups)"
    )
    run.add_argument("groups", nargs="*")

    gate = subparsers.add_parser(
        "gate",
        help="finalize a lane, mark missing probes blocked, and exit non-zero on red",
    )
    gate.add_argument("groups", nargs="*")

    rerun = subparsers.add_parser(
        "rerun-failed",
        help="rerun failed probes and blocked probes that have become reachable",
    )
    rerun.add_argument("report")

    merge = subparsers.add_parser(
        "merge", help="merge lane reports/diagnostic directories into one report"
    )
    merge.add_argument("paths", nargs="+")

    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    try:
        manifest_path = _resolve_path(args.manifest)
        manifest = load_manifest(manifest_path)
        output_dir = _resolve_path(args.output)

        if args.command == "run-check":
            return command_run_check(manifest, output_dir, args.check_id)
        if args.command == "run":
            return command_run(manifest, output_dir, args.groups)
        if args.command == "gate":
            return command_gate(manifest, output_dir, args.groups)
        if args.command == "rerun-failed":
            return command_rerun_failed(
                manifest, output_dir, _resolve_path(args.report)
            )
        if args.command == "merge":
            return command_merge(manifest, output_dir, args.paths)
        raise DiagnosticError(f"unknown command {args.command!r}")
    except DiagnosticError as exc:
        print(f"diagnose: {exc}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("diagnose: interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
