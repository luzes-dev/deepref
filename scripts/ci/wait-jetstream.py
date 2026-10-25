#!/usr/bin/env python3
"""Bounded, fail-closed JetStream health wait for the owned CI service."""
import json
import sys
import time
import urllib.error
import urllib.request


def wait(port: int, budget: float = 45) -> None:
    deadline = time.monotonic() + budget
    url = f"http://127.0.0.1:{port}/healthz?js-enabled-only=true"
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=min(1, max(0.01, deadline-time.monotonic()))) as response:
                if response.status == 200 and json.load(response).get("status") == "ok":
                    return
        except urllib.error.HTTPError as error:
            error.close()
        except (OSError, ValueError, urllib.error.URLError):
            pass
        time.sleep(min(0.2, max(0, deadline-time.monotonic())))
    raise SystemExit("JetStream readiness failed before deadline")


if __name__ == "__main__":
    wait(int(sys.argv[1]))
