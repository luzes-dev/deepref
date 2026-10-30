"""Real owned subprocess groups; no services, Cargo or shared fixture lock."""

import errno
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/operations"))
import test_observability as runner

LEADER = r'''
from pathlib import Path
import subprocess
import sys
import time

subprocess.Popen([sys.executable, "-c",
    "import os,signal,sys,time; from pathlib import Path; "
    "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
    "Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(10)", sys.argv[1]])
deadline = time.monotonic() + 2
while not Path(sys.argv[1]).exists():
    if time.monotonic() > deadline:
        raise RuntimeError("descendant did not become ready")
    time.sleep(0.01)
if sys.argv[2] == "timeout":
    time.sleep(10)
'''


def group_exists(group):
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    return True


class PhaseLifetimeTests(unittest.TestCase):
    def capture(self, mode):
        receipt = {}
        failure = None
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryFile() as lock:
            root = Path(directory)
            pidfile = root / "descendant.pid"
            previous = None
            try:
                try:
                    previous = os.dup(9)
                except OSError as error:
                    if error.errno != errno.EBADF:
                        raise
                os.dup2(lock.fileno(), 9)
                command = [sys.executable, "-c", "pass"] if mode == "clean" else [
                    sys.executable, "-c", LEADER, str(pidfile), mode]
                try:
                    try:
                        runner.run_phase(mode, command, output=root, environment=dict(os.environ),
                                         deadline=time.monotonic() + 6, receipt=receipt)
                    except Exception as error:
                        failure = error
                    if mode != "clean":
                        self.assertTrue(pidfile.exists(), "descendant never reached readiness")
                    group = receipt["process_group"]
                    survived = group_exists(group)
                finally:
                    group = receipt.get("process_group")
                    if group is not None and group_exists(group):
                        os.killpg(group, signal.SIGKILL)
                    deadline = time.monotonic() + 2
                    while group is not None and group_exists(group) and time.monotonic() < deadline:
                        time.sleep(0.01)
                    if group is not None and group_exists(group):
                        raise RuntimeError("test-owned phase group remains")
            finally:
                if previous is not None:
                    os.dup2(previous, 9)
                    os.close(previous)
                else:
                    os.close(9)
        print("OWNED_GROUP_CLOSED " + json.dumps(receipt), flush=True)
        return receipt, failure, survived

    def test_timeout_reaps_group_even_when_leader_exits_on_term(self):
        receipt, failure, survived = self.capture("timeout")
        self.assertIsInstance(failure, subprocess.TimeoutExpired)
        self.assertTrue(receipt["process_exited"])
        self.assertFalse(survived, "phase descendant survived after its leader exited on TERM")

    def test_successful_leader_cannot_leave_a_background_descendant(self):
        _, failure, survived = self.capture("early-exit")
        self.assertIsInstance(failure, RuntimeError)
        self.assertFalse(survived, "successful phase leader left a descendant running")

    def test_clean_phase_completes_without_residual_group(self):
        receipt, failure, survived = self.capture("clean")
        self.assertIsNone(failure)
        self.assertEqual(receipt["exit"], 0)
        self.assertFalse(survived)


if __name__ == "__main__":
    unittest.main()
