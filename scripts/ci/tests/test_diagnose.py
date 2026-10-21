from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

DIAGNOSE = Path(__file__).resolve().parents[1] / "diagnose.py"
REPO_ROOT = Path(__file__).resolve().parents[3]


def python_probe(source: str, *arguments: str) -> list[str]:
    return [sys.executable, "-c", source, *arguments]


class DiagnoseCliTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.output = self.root / "diagnostics"
        self.manifest = self.root / "diagnostics.toml"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_manifest(
        self,
        checks: list[dict[str, object]],
        *,
        excerpt_lines: int | None = None,
        excerpt_chars: int | None = None,
    ) -> None:
        lines: list[str] = []
        if excerpt_lines is not None or excerpt_chars is not None:
            lines.append("[report]")
            if excerpt_lines is not None:
                lines.append(f"excerpt_lines = {excerpt_lines}")
            if excerpt_chars is not None:
                lines.append(f"excerpt_chars = {excerpt_chars}")
            lines.append("")
        for check in checks:
            lines.append("[[check]]")
            lines.append(f"id = {json.dumps(check['id'])}")
            lines.append(f"group = {json.dumps(check['group'])}")
            command = ", ".join(json.dumps(part) for part in check["command"])
            lines.append(f"command = [{command}]")
            depends_on = check.get("depends_on", [])
            if depends_on:
                deps = ", ".join(json.dumps(dep) for dep in depends_on)
                lines.append(f"depends_on = [{deps}]")
            env = check.get("env", {})
            if env:
                values = ", ".join(
                    f"{json.dumps(key)} = {json.dumps(value)}"
                    for key, value in env.items()
                )
                lines.append(f"env = {{ {values} }}")
            lines.append("")
        self.manifest.write_text("\n".join(lines), encoding="utf-8")

    def cli(
        self,
        *arguments: str,
        output: Path | None = None,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        command = [
            sys.executable,
            str(DIAGNOSE),
            "--manifest",
            str(self.manifest),
            "--output",
            str(output or self.output),
            *arguments,
        ]
        environment = os.environ.copy()
        environment.pop("GITHUB_STEP_SUMMARY", None)
        if env:
            environment.update(env)
        return subprocess.run(
            command,
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            env=environment,
            check=False,
        )

    def read_result(self, check_id: str, output: Path | None = None) -> dict[str, object]:
        path = (output or self.output) / "checks" / f"{check_id}.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def read_report(self, output: Path | None = None) -> dict[str, object]:
        path = (output or self.output) / "report.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def test_passing_probe_writes_result_and_report(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.pass",
                    "group": "sample",
                    "command": python_probe("print('probe output')"),
                }
            ]
        )

        completed = self.cli("run", "sample")

        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = self.read_result("sample.pass")
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["exit_code"], 0)
        self.assertTrue((self.output / result["log"]).is_file())
        report = self.read_report()
        self.assertEqual(report["summary"]["passed"], 1)
        self.assertEqual(report["probes"][0]["id"], "sample.pass")
        self.assertIn("| `sample.pass` | **PASS** |", (self.output / "report.md").read_text())

    def test_failing_probe_is_recorded(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.fail",
                    "group": "sample",
                    "command": python_probe("print('boom'); raise SystemExit(1)"),
                }
            ]
        )

        completed = self.cli("run", "sample")

        self.assertNotEqual(completed.returncode, 0)
        result = self.read_result("sample.fail")
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["exit_code"], 1)
        self.assertIn("boom", (self.output / result["log"]).read_text())
        report = self.read_report()
        self.assertEqual(report["summary"]["failed"], 1)
        self.assertIn("## Failed probes", (self.output / "report.md").read_text())

    def test_multiple_independent_failures_are_all_reported(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.first",
                    "group": "sample",
                    "command": python_probe("print('first'); raise SystemExit(1)"),
                },
                {
                    "id": "sample.second",
                    "group": "sample",
                    "command": python_probe("print('second'); raise SystemExit(2)"),
                },
            ]
        )

        completed = self.cli("run", "sample")

        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(self.read_result("sample.first")["status"], "failed")
        self.assertEqual(self.read_result("sample.second")["status"], "failed")
        self.assertEqual(self.read_report()["summary"]["failed"], 2)

    def test_dependency_failure_blocks_child_without_executing_it(self) -> None:
        marker = self.root / "child-ran"
        self.write_manifest(
            [
                {
                    "id": "sample.parent",
                    "group": "sample",
                    "command": python_probe("raise SystemExit(1)"),
                },
                {
                    "id": "sample.child",
                    "group": "sample",
                    "command": python_probe(
                        "from pathlib import Path; import sys; "
                        "Path(sys.argv[1]).write_text('ran')",
                        str(marker),
                    ),
                    "depends_on": ["sample.parent"],
                },
            ]
        )

        completed = self.cli("run", "sample")

        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(self.read_result("sample.parent")["status"], "failed")
        child = self.read_result("sample.child")
        self.assertEqual(child["status"], "blocked")
        self.assertFalse(marker.exists())
        self.assertIn("blocked", (self.output / child["log"]).read_text())
        self.assertEqual(self.read_report()["summary"]["blocked"], 1)

    def test_dependency_runs_after_prerequisite_succeeds(self) -> None:
        marker = self.root / "child-ran"
        self.write_manifest(
            [
                {
                    "id": "sample.parent",
                    "group": "sample",
                    "command": python_probe("print('parent')"),
                },
                {
                    "id": "sample.child",
                    "group": "sample",
                    "command": python_probe(
                        "from pathlib import Path; import sys; "
                        "Path(sys.argv[1]).write_text('ran')",
                        str(marker),
                    ),
                    "depends_on": ["sample.parent"],
                },
            ]
        )

        completed = self.cli("run", "sample")

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.read_result("sample.parent")["status"], "passed")
        self.assertEqual(self.read_result("sample.child")["status"], "passed")
        self.assertTrue(marker.exists())

    def test_full_log_retained_and_markdown_excerpt_bounded(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.noisy",
                    "group": "sample",
                    "command": python_probe(
                        "import sys\n"
                        "for index in range(200):\n"
                        "    print(f'error line {index}')\n"
                        "print('\\x1b[31mANSI error: final failure\\x1b[0m')\n"
                        "raise SystemExit(1)"
                    ),
                }
            ],
            excerpt_lines=12,
            excerpt_chars=500,
        )

        completed = self.cli("run", "sample")

        self.assertNotEqual(completed.returncode, 0)
        result = self.read_result("sample.noisy")
        raw_log = (self.output / result["log"]).read_text(encoding="utf-8")
        self.assertEqual(len(raw_log.splitlines()), 201)
        self.assertIn("\x1b[31m", raw_log)
        markdown = (self.output / "report.md").read_text(encoding="utf-8")
        self.assertNotIn("\x1b[", markdown)
        self.assertIn("lines omitted; see the full log", markdown)
        self.assertIn(f"`{result['log']}`", markdown)
        self.assertLess(len(markdown), 4000)

    def test_rerun_failed_only_reruns_failed_and_newly_reachable_probes(self) -> None:
        marker = self.root / "fixed"
        keep_counter = self.root / "keep.count"
        parent_counter = self.root / "parent.count"
        child_counter = self.root / "child.count"

        counter_probe = (
            "from pathlib import Path\n"
            "import sys\n"
            "Path(sys.argv[1]).open('a', encoding='utf-8').write('x')\n"
            "raise SystemExit(0 if Path(sys.argv[2]).exists() else 1)"
        )
        self.write_manifest(
            [
                {
                    "id": "sample.keep",
                    "group": "sample",
                    "command": python_probe(
                        "from pathlib import Path; import sys; "
                        "Path(sys.argv[1]).open('a', encoding='utf-8').write('x')",
                        str(keep_counter),
                    ),
                },
                {
                    "id": "sample.parent",
                    "group": "sample",
                    "command": python_probe(
                        counter_probe, str(parent_counter), str(marker)
                    ),
                },
                {
                    "id": "sample.child",
                    "group": "sample",
                    "command": python_probe(
                        counter_probe, str(child_counter), str(marker)
                    ),
                    "depends_on": ["sample.parent"],
                },
            ]
        )

        first = self.cli("run", "sample")
        self.assertNotEqual(first.returncode, 0)
        self.assertEqual(self.read_result("sample.parent")["status"], "failed")
        self.assertEqual(self.read_result("sample.child")["status"], "blocked")
        self.assertEqual(keep_counter.read_text(), "x")
        self.assertEqual(parent_counter.read_text(), "x")
        self.assertFalse(child_counter.exists())

        marker.write_text("fixed")
        second = self.cli("rerun-failed", str(self.output))

        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(keep_counter.read_text(), "x")
        self.assertEqual(parent_counter.read_text(), "xx")
        self.assertEqual(child_counter.read_text(), "x")
        self.assertEqual(self.read_result("sample.parent")["status"], "passed")
        self.assertEqual(self.read_result("sample.child")["status"], "passed")

    def test_merge_combines_lane_reports_and_copies_logs(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.pass",
                    "group": "alpha",
                    "command": python_probe("print('alpha ok')"),
                },
                {
                    "id": "sample.fail",
                    "group": "beta",
                    "command": python_probe("print('beta bad'); raise SystemExit(1)"),
                },
            ]
        )
        alpha = self.root / "alpha"
        beta = self.root / "beta"
        merged = self.root / "merged"
        self.assertEqual(self.cli("run", "alpha", output=alpha).returncode, 0)
        self.assertNotEqual(self.cli("run", "beta", output=beta).returncode, 0)

        completed = self.cli(
            "merge",
            str(alpha),
            str(beta / "report.json"),
            output=merged,
        )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        report = self.read_report(merged)
        self.assertEqual(
            [probe["id"] for probe in report["probes"]],
            ["sample.pass", "sample.fail"],
        )
        self.assertEqual(report["summary"]["failed"], 1)
        for probe in report["probes"]:
            self.assertTrue((merged / probe["log"]).is_file())

    def test_gate_marks_missing_probe_blocked_and_exits_non_zero(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.pass",
                    "group": "sample",
                    "command": python_probe("print('ok')"),
                },
                {
                    "id": "sample.not-reached",
                    "group": "sample",
                    "command": python_probe("print('should not run')"),
                },
            ]
        )
        self.assertEqual(
            self.cli("run-check", "sample.pass").returncode, 0
        )

        completed = self.cli("gate", "sample")

        self.assertNotEqual(completed.returncode, 0)
        missing = self.read_result("sample.not-reached")
        self.assertEqual(missing["status"], "blocked")
        self.assertIn("not reached", missing["error"])
        self.assertEqual(self.read_report()["summary"]["blocked"], 1)

    def test_gate_passes_when_every_probe_passed(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.pass",
                    "group": "sample",
                    "command": python_probe("print('ok')"),
                }
            ]
        )
        self.assertEqual(self.cli("run", "sample").returncode, 0)

        completed = self.cli("gate", "sample")

        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_run_check_blocks_on_failed_dependency(self) -> None:
        marker = self.root / "child-ran"
        self.write_manifest(
            [
                {
                    "id": "sample.parent",
                    "group": "sample",
                    "command": python_probe("raise SystemExit(1)"),
                },
                {
                    "id": "sample.child",
                    "group": "sample",
                    "command": python_probe(
                        "from pathlib import Path; import sys; "
                        "Path(sys.argv[1]).write_text('ran')",
                        str(marker),
                    ),
                    "depends_on": ["sample.parent"],
                },
            ]
        )
        self.assertNotEqual(
            self.cli("run-check", "sample.parent").returncode, 0
        )

        completed = self.cli("run-check", "sample.child")

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.read_result("sample.child")["status"], "blocked")
        self.assertFalse(marker.exists())

    def test_missing_executable_is_a_failed_probe(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.missing",
                    "group": "sample",
                    "command": ["definitely-not-a-deepref-command"],
                }
            ]
        )

        completed = self.cli("run", "sample")

        self.assertNotEqual(completed.returncode, 0)
        result = self.read_result("sample.missing")
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["exit_code"], 127)
        self.assertIn("executable not found", result["error"])
        self.assertIn("executable not found", (self.output / result["log"]).read_text())

    def test_malformed_manifest_is_an_infrastructure_error(self) -> None:
        self.manifest.write_text("[[check]\nid = 'broken'\n", encoding="utf-8")

        completed = self.cli("run", "sample")

        self.assertEqual(completed.returncode, 2)
        self.assertIn("malformed manifest", completed.stderr)

    def test_unknown_dependency_is_a_manifest_error(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.child",
                    "group": "sample",
                    "command": python_probe("print('ok')"),
                    "depends_on": ["sample.missing"],
                }
            ]
        )

        completed = self.cli("run", "sample")

        self.assertEqual(completed.returncode, 2)
        self.assertIn("unknown check", completed.stderr)

    def test_dependency_cycle_is_a_manifest_error(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.first",
                    "group": "sample",
                    "command": python_probe("print('first')"),
                    "depends_on": ["sample.second"],
                },
                {
                    "id": "sample.second",
                    "group": "sample",
                    "command": python_probe("print('second')"),
                    "depends_on": ["sample.first"],
                },
            ]
        )

        completed = self.cli("run", "sample")

        self.assertEqual(completed.returncode, 2)
        self.assertIn("dependency cycle", completed.stderr)

    def test_report_structure_is_deterministic(self) -> None:
        self.write_manifest(
            [
                {
                    "id": "sample.zeta",
                    "group": "sample",
                    "command": python_probe("print('zeta')"),
                },
                {
                    "id": "sample.alpha",
                    "group": "sample",
                    "command": python_probe("print('alpha')"),
                },
            ]
        )

        completed = self.cli("run", "sample")

        self.assertEqual(completed.returncode, 0, completed.stderr)
        report = self.read_report()
        self.assertEqual(
            [probe["id"] for probe in report["probes"]],
            ["sample.zeta", "sample.alpha"],
        )
        self.assertEqual(
            set(report), {"schema_version", "groups", "summary", "probes"}
        )
        self.assertEqual(report["groups"], ["sample"])
        for probe in report["probes"]:
            self.assertTrue(
                {"id", "group", "status", "exit_code", "duration_ms", "log"}
                <= set(probe)
            )


    def recovery_manifest(self) -> None:
        self.write_manifest([
            {"id": "sample.parent", "group": "sample", "command": python_probe("print('parent')")},
            {"id": "sample.child", "group": "sample", "command": python_probe("print('child')"),
             "depends_on": ["sample.parent"]},
            {"id": "other.root", "group": "other", "command": python_probe("print('unrelated')")},
        ])

    def test_rerunning_parent_invalidates_previously_passed_descendant(self) -> None:
        self.recovery_manifest()
        self.assertEqual(self.cli("run", "sample").returncode, 0)
        self.assertEqual(self.cli("run-check", "sample.parent").returncode, 0)
        self.assertNotEqual(self.cli("gate", "sample").returncode, 0)
        self.assertEqual(self.read_result("sample.child")["status"], "blocked")

    def test_fresh_output_rerun_persists_preserved_results_for_gate(self) -> None:
        self.recovery_manifest()
        self.assertEqual(self.cli("run", "sample").returncode, 0)
        fresh = self.root / "fresh"
        completed = self.cli("rerun-failed", str(self.output), output=fresh)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.cli("gate", "sample", output=fresh).returncode, 0)
        self.assertEqual(self.read_result("sample.child", fresh)["status"], "passed")

    def test_rerun_recovers_setup_only_blocked_roots(self) -> None:
        self.recovery_manifest()
        self.assertNotEqual(self.cli("gate", "other").returncode, 0)
        completed = self.cli("rerun-failed", str(self.output))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.read_result("other.root")["status"], "passed")
        self.assertEqual(self.cli("gate", "other").returncode, 0)

    def test_excerpt_character_and_line_limits_include_truncation_marker(self) -> None:
        self.write_manifest([
            {"id": "sample.fail", "group": "sample",
             "command": python_probe("print('x' * 1000); raise SystemExit(1)")},
        ], excerpt_chars=20, excerpt_lines=1)
        self.assertNotEqual(self.cli("run", "sample").returncode, 0)
        markdown = (self.output / "report.md").read_text()
        excerpt = markdown.split("~~~~text\n", 1)[1].split("\n~~~~", 1)[0]
        self.assertLessEqual(len(excerpt), 20)
        self.assertLessEqual(len(excerpt.splitlines()), 1)

    def test_merge_marks_expected_missing_lane_blocked_without_becoming_gate(self) -> None:
        self.recovery_manifest()
        artifact = self.root / "empty-artifacts"
        artifact.mkdir()
        completed = self.cli("merge", str(artifact), "--expect-group", "sample")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.read_report()["summary"]["blocked"], 2)
        self.assertEqual(self.read_report()["summary"]["failed"], 0)

    def test_merge_retains_distinct_failed_logs_for_same_probe(self) -> None:
        self.write_manifest([
            {"id": "sample.fail", "group": "sample",
             "command": python_probe("import os; print(os.environ['EVIDENCE']); raise SystemExit(1)")},
        ])
        first = self.root / "first"
        second = self.root / "second"
        self.assertNotEqual(self.cli("run", output=first, env={"EVIDENCE": "first failure"}).returncode, 0)
        self.assertNotEqual(self.cli("run", output=second, env={"EVIDENCE": "second failure"}).returncode, 0)
        completed = self.cli("merge", str(first), str(second))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = self.read_result("sample.fail")
        log = (self.output / result["log"]).read_text()
        self.assertIn("first failure", log)
        self.assertIn("second failure", log)
        self.assertEqual(log.count("first failure"), 1)
        self.assertEqual(log.count("second failure"), 1)

    def test_manifest_rejects_invalid_environment_without_traceback(self) -> None:
        for env in ({"INVALID=KEY": "value"}, {"KEY": "\0"}):
            with self.subTest(env=env):
                self.write_manifest([
                    {"id": "sample.pass", "group": "sample",
                     "command": python_probe("print('ok')"), "env": env},
                ])
                completed = self.cli("run")
                self.assertEqual(completed.returncode, 2)
                self.assertIn("invalid env", completed.stderr)
                self.assertNotIn("Traceback", completed.stderr)

    def test_empty_argv_argument_is_preserved(self) -> None:
        self.write_manifest([
            {"id": "sample.argv", "group": "sample",
             "command": python_probe("import sys; assert sys.argv[1] == ''", "")},
        ])
        completed = self.cli("run")
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_manifest_rejects_newline_ids_and_nul_argv(self) -> None:
        for check_id, command in (
            ("sample.bad\n", python_probe("print('ok')")),
            ("sample.bad", python_probe("print('ok')", "\0")),
        ):
            with self.subTest(check_id=check_id, command=command):
                self.write_manifest([
                    {"id": check_id, "group": "sample", "command": command},
                ])
                completed = self.cli("run")
                self.assertEqual(completed.returncode, 2)
                self.assertIn("malformed manifest", completed.stderr)
                self.assertNotIn("Traceback", completed.stderr)

    def test_expected_lane_preserves_failure_while_filling_missing_probes(self) -> None:
        self.write_manifest([
            {"id": "sample.fail", "group": "sample",
             "command": python_probe("raise SystemExit(1)")},
            {"id": "sample.missing", "group": "sample",
             "command": python_probe("print('not run')")},
        ])
        source = self.root / "partial"
        self.assertNotEqual(self.cli("run-check", "sample.fail", output=source).returncode, 0)
        completed = self.cli("merge", str(source), "--expect-group", "sample")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.read_report()["summary"]["failed"], 1)
        self.assertEqual(self.read_report()["summary"]["blocked"], 1)



if __name__ == "__main__":
    unittest.main()
