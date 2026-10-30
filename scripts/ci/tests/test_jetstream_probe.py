"""Readiness and real-workflow contract for the required NATS probe."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("wait_jetstream", ROOT / "scripts/ci/wait-jetstream.py")
readiness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(readiness)


class ReadinessTests(unittest.TestCase):
    def serve(self, status, body):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(status)
                self.end_headers()
                self.wfile.write(json.dumps(body).encode())

            def log_message(self, *_):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return server.server_port

    def test_jetstream_ready(self):
        readiness.wait(self.serve(200, {"status": "ok"}), budget=0.2)

    def test_unready_fails_closed_with_bounded_wait(self):
        port = self.serve(503, {"status": "unavailable"})
        started = time.monotonic()
        with self.assertRaises(SystemExit):
            readiness.wait(port, budget=0.1)
        self.assertLess(time.monotonic() - started, 0.5)

    def test_missing_service_fails_closed(self):
        with self.assertRaises(SystemExit):
            readiness.wait(0, budget=0.05)

    def test_required_probe_is_enabled_in_real_integration_workflow(self):
        manifest = tomllib.loads((ROOT / "scripts/ci/diagnostics.toml").read_text())
        probe = next(check for check in manifest["check"] if check["id"] == "integration.nats")
        self.assertEqual(probe["group"], "integration")
        self.assertEqual(probe["command"], ["bash", "scripts/ci/test-jetstream.sh"])
        workflow = (ROOT / ".github/workflows/ci-integration.yml").read_text()
        self.assertIn("diagnose.py run-check integration.nats", workflow)
        self.assertIn("diagnose.py gate integration", workflow)
        runner = (ROOT / "scripts/ci/test-jetstream.sh").read_text()
        self.assertIn("DEEPREF_REQUIRE_NATS=1", runner)
        self.assertIn("wait-jetstream.py", runner)
        self.assertIn("--test jetstream_review", runner)


class FixtureStartupTests(unittest.TestCase):
    def run_probe(self, *, never_ready=False):
        # Exercise the public shell command while Docker models the image's
        # temporary socket-only init server, restart gap, then TCP readiness.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = root / "state.json"
            shim = root / "shim"
            shim.write_text(f"#!{sys.executable}\n" + r'''
import json
import os
from pathlib import Path
import sys

name = Path(sys.argv[0]).name
args = sys.argv[1:]
path = Path(os.environ["PROBE_STATE"])
status = 0
if name == "docker":
    state = json.loads(path.read_text()) if path.exists() else {}
    if args[0] == "exec" and "pg_isready" in args:
        if "-h" in args and args[args.index("-h") + 1] == "127.0.0.1":
            state["tcp_checks"] = state.get("tcp_checks", 0) + 1
            ready = state["tcp_checks"] >= 3 and not os.environ.get("NEVER_READY")
            state["tcp_ready"] = bool(ready)
            status = 0 if ready else 2
        else:
            state["socket_checks"] = state.get("socket_checks", 0) + 1
            status = 0 if state["socket_checks"] == 1 else 2
    elif args[0] == "port":
        print("127.0.0.1:12345")
    elif args[0] == "rm":
        state["cleaned"] = True
    path.write_text(json.dumps(state))
elif name == "python3":
    # The broker wait must not start before PostgreSQL's final TCP server.
    state = json.loads(path.read_text()) if path.exists() else {}
    status = 0 if state.get("tcp_ready") else 1
elif name == "awk":
    if args[-1] == "/proc/meminfo":
        print(6291456)
    else:
        os.execv(os.environ["PROBE_AWK"], ["awk", *args])
sys.exit(status)
''')
            shim.chmod(0o700)
            for name in ["docker", "find", "flock", "sleep", "python3", "awk"]:
                (root / name).symlink_to(shim)
            env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                       PROBE_STATE=str(state), DEEPREF_IMPL_CACHE=str(root),
                       PROBE_AWK=shutil.which("awk"))
            if never_ready:
                env["NEVER_READY"] = "1"
            marker = root / "test-command-ran"
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/ci/test-jetstream.sh"),
                 "touch", str(marker)], env=env, capture_output=True, text=True,
                timeout=10,
            )
            return result, json.loads(state.read_text()), marker.exists()

    def test_temporary_socket_readiness_does_not_start_tests(self):
        result, state, ran = self.run_probe()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(ran)
        self.assertTrue(state["tcp_ready"])
        self.assertGreaterEqual(state["tcp_checks"], 3)
        self.assertTrue(state["cleaned"])

    def test_missing_tcp_server_fails_closed_and_cleans_up(self):
        result, state, ran = self.run_probe(never_ready=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("PostgreSQL TCP readiness failed before deadline", result.stderr)
        self.assertFalse(ran)
        self.assertEqual(state["tcp_checks"], 45)
        self.assertTrue(state["cleaned"])


if __name__ == "__main__":
    unittest.main()
