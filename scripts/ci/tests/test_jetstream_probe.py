"""Readiness and real-workflow contract for the required NATS probe."""
import importlib.util
import json
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
