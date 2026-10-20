#!/usr/bin/env python3
"""Require immutable external references in GitHub workflows and actions.

Policy:
  * External GitHub Actions must use a full 40-character commit SHA plus a
    readable version comment (for example ``# v7.0.1``).
  * Local action/workflow references (``./...``) are allowed.
  * ``docker://`` references must be pinned by digest instead of a tag.

The scanner is line based so it does not depend on a YAML parser being
installed on the runner. It skips block scalar bodies (``run: |`` etc.) so
shell text that happens to contain ``uses:`` is not treated as an action
reference.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

USES_PATTERN = re.compile(
    r"^\s*(?:-\s+)?uses:\s*(?P<ref>\S+?)\s*(?:#\s*(?P<comment>.*?))?\s*$"
)
BLOCK_SCALAR_PATTERN = re.compile(
    r"^\s*(?:-\s+)?[A-Za-z0-9_.-]+:\s*[|>][+-]?\s*(?:#.*)?$"
)
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
DIGEST_PATTERN = re.compile(r"^[0-9a-f]{64}$")
VERSION_COMMENT_PATTERN = re.compile(r"^v?\d")


def iter_reference_files(root: Path) -> list[Path]:
    files = sorted(root.glob(".github/workflows/*.yml"))
    files += sorted(root.glob(".github/workflows/*.yaml"))
    files += sorted(root.glob(".github/actions/*/action.yml"))
    files += sorted(root.glob(".github/actions/*/action.yaml"))
    return files


def validate_reference(
    reference: str, comment: str | None
) -> tuple[bool, str | None]:
    if reference.startswith("./"):
        return True, None

    if reference.startswith("docker://"):
        image = reference.removeprefix("docker://")
        if "@sha256:" not in image:
            return False, "container references must be pinned by sha256 digest"
        digest = image.rsplit("@sha256:", 1)[1]
        if not DIGEST_PATTERN.match(digest):
            return False, "container digest must be a 64-character lowercase hex value"
        return True, None

    if "@" not in reference:
        return False, "external references must include @<40-character commit SHA>"

    action, revision = reference.rsplit("@", 1)
    if not action:
        return False, "missing action name before @"
    if revision.startswith("sha256:"):
        return False, "use a full 40-character commit SHA, not a digest"
    if not SHA_PATTERN.match(revision):
        return False, "revision must be a full 40-character lowercase commit SHA"
    if not comment:
        return False, "missing readable version comment (for example '# v1.2.3')"
    if not VERSION_COMMENT_PATTERN.match(comment.strip()):
        return False, "version comment must start with a version number or 'v'"

    return True, None


def scan_file(path: Path, display_path: Path) -> list[str]:
    errors: list[str] = []
    block_indent: int | None = None

    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        stripped = line.strip()
        indent = len(line) - len(line.lstrip(" "))

        if block_indent is not None:
            if not stripped or indent > block_indent:
                continue
            block_indent = None

        if BLOCK_SCALAR_PATTERN.match(line):
            block_indent = indent
            continue

        match = USES_PATTERN.match(line)
        if not match:
            continue

        reference = match.group("ref").strip("\"'")
        comment = match.group("comment")
        valid, reason = validate_reference(reference, comment)
        if not valid:
            errors.append(f"{display_path}:{number}: {stripped}: {reason}")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path.cwd(),
        help="repository root to scan (default: current directory)",
    )
    args = parser.parse_args()

    root = args.root.resolve()
    files = iter_reference_files(root)
    if not files:
        print(f"error: no workflow or action files found under {root}", file=sys.stderr)
        return 2

    errors: list[str] = []
    for path in files:
        errors.extend(scan_file(path, path.relative_to(root)))

    if errors:
        print(
            "Every external action must use a full 40-character SHA and a readable "
            "version comment:",
            file=sys.stderr,
        )
        print("\n".join(errors), file=sys.stderr)
        return 1

    print(f"action pin policy passed: {len(files)} workflow/action files checked")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
