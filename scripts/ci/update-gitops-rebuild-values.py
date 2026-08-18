#!/usr/bin/env python3
"""Apply one reviewed projector-rebuild state transition to a GitOps values file."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


UUID_PATTERN = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)
REBUILD = re.compile(
    r'(?m)^(rebuild:\n)([ \t]+enabled: )(true|false)(\n)([ \t]+runId: )"([^"]*)"(\n)'
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("values_file", type=Path)
    parser.add_argument("mode", choices=("start", "stop"))
    parser.add_argument("run_id")
    args = parser.parse_args()

    if args.mode == "start" and not UUID_PATTERN.fullmatch(args.run_id):
        raise SystemExit("start run_id must be a lowercase RFC 4122 UUID")
    if args.mode == "stop" and args.run_id != "STOP":
        raise SystemExit("stop run_id must be STOP")

    text = args.values_file.read_text()
    match = REBUILD.search(text)
    if match is None:
        raise SystemExit("values file must contain the expected rebuild block")

    current_enabled = match.group(3)
    current_run_id = match.group(6)
    if args.mode == "start":
        if current_enabled != "false" or current_run_id:
            raise SystemExit("rebuild is already enabled or has a non-empty runId")
        enabled, run_id = "true", args.run_id
    else:
        if current_enabled != "true" or not UUID_PATTERN.fullmatch(current_run_id):
            raise SystemExit("rebuild is not enabled with a valid runId")
        enabled, run_id = "false", ""

    replacement = (
        f"{match.group(1)}{match.group(2)}{enabled}{match.group(4)}"
        f"{match.group(5)}\"{run_id}\"{match.group(7)}"
    )
    args.values_file.write_text(text[: match.start()] + replacement + text[match.end() :])


if __name__ == "__main__":
    main()
