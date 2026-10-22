"""Behavior regressions for the dependency-free action pin policy CLI."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCANNER = Path(__file__).with_name("check-action-pins.py")
PINNED = "actions/checkout@" + "a" * 40 + " # v7.0.1"
HEADERS = ("|", "|-", "|+", ">", ">-", ">+", "|2", "|2-", "|+2", ">2", ">2+", ">-2")


class ActionPinCliTests(unittest.TestCase):
    def check_steps(self, steps: str, expected: int, diagnostic: str = "") -> None:
        fixtures = {
            ".github/workflows/test.yml": (
                "name: Test\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n",
                6,
            ),
            ".github/actions/test/action.yaml": (
                "name: Test\ndescription: Test\nruns:\n  using: composite\n  steps:\n",
                4,
            ),
        }
        for filename, (prefix, indent) in fixtures.items():
            with self.subTest(file=filename), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                path = root / filename
                path.parent.mkdir(parents=True)
                path.write_text(
                    prefix + "\n".join(" " * indent + line for line in steps.splitlines()) + "\n",
                    encoding="utf-8",
                )
                result = subprocess.run(
                    [sys.executable, str(SCANNER), "--root", str(root)],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                if diagnostic:
                    self.assertIn(diagnostic, result.stderr)
                    self.assertIn(filename, result.stderr)

    def test_block_scalar_references_are_rejected(self):
        for header in HEADERS:
            with self.subTest(header=header):
                self.check_steps(
                    f"- uses: {header}\n    actions/checkout@v7",
                    1,
                    "action references must be on one line",
                )

    def test_multiline_names_do_not_hide_sibling_references(self):
        for header in HEADERS:
            for reference, status in (("actions/checkout@v7", 1), (PINNED, 0)):
                with self.subTest(header=header, reference=reference):
                    self.check_steps(
                        f"- name: {header}\n    Checkout sources\n  uses: {reference}",
                        status,
                    )

    def test_external_references_require_sha_and_comment(self):
        for reference, diagnostic in (
            ("actions/checkout", "external references must include"),
            ("actions/checkout@v7 # v7", "revision must be a full"),
            ("actions/checkout@" + "a" * 40, "missing readable version comment"),
            ("actions/checkout@" + "A" * 40 + " # v7", "revision must be a full"),
        ):
            with self.subTest(reference=reference):
                self.check_steps(f"- uses: {reference}", 1, diagnostic)

    def test_run_scalar_contents_are_not_references(self):
        for header in HEADERS:
            for opener, body_indent in (("- run:", "    "), ("- name: Shell\n  run:", "    ")):
                with self.subTest(header=header, opener=opener):
                    self.check_steps(
                        f"{opener} {header}\n"
                        f"{body_indent}uses: actions/checkout@v7\n"
                        f"{body_indent}- uses: >-\n"
                        f"{body_indent}  actions/checkout@v7\n"
                        f"- uses: {PINNED}",
                        0,
                    )

    def test_local_and_digest_references_remain_allowed(self):
        self.check_steps(
            "- uses: ./.github/actions/test\n"
            "- uses: docker://alpine@sha256:" + "b" * 64,
            0,
        )
        self.check_steps("- uses: docker://alpine:latest", 1, "pinned by sha256 digest")


if __name__ == "__main__":
    unittest.main()
