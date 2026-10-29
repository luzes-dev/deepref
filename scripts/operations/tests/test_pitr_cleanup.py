"""Owned Docker lifecycle regressions; never run without an explicit fixture slot.

The command adapter loses its creation reply after real Docker creation, leaving
an incomplete CID file. No PostgreSQL recovery result is inferred from this test.
"""

import copy
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import unittest
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import pitr_local as pitr


class LostCreateReply(RuntimeError):
    pass


class InterruptedCreateDrill(pitr.Drill):
    """Inject failure at the external command-result boundary, not cleanup."""

    def __init__(self, run, identity, cid_state):
        super().__init__(run, identity)
        self.cid_state = cid_state
        self.committed_id = None

    def command(self, label, args, **kwargs):
        completed = super().command(label, args, **kwargs)
        if label == "start-source":
            self.committed_id = pitr.bounded_identifier(
                completed.stdout.strip(), r"[a-f0-9]{64}"
            )
            cidfile = Path(self.containers["source"]["cidfile"])
            if self.cid_state == "empty":
                cidfile.write_bytes(b"")
            elif self.cid_state == "partial":
                cidfile.write_bytes(self.committed_id[:12].encode("ascii"))
            elif self.cid_state == "non_ascii":
                cidfile.write_bytes(b"\xff")
            elif self.cid_state == "absent":
                cidfile.unlink()
            elif self.cid_state != "valid":
                raise ValueError("unknown test CID state")
            raise LostCreateReply("controlled loss after actual owned Docker creation")
        return completed


def owned_inventory(drill):
    """Require healthy catalog queries, independent of failed CID-file parsing."""
    result = {}
    for kind in ("container", "volume", "network"):
        args = [kind, "ls", "--filter", "label=deepref.task=" + drill.label]
        if kind == "container":
            args += ["--all", "--no-trunc"]
        elif kind == "network":
            args += ["--no-trunc"]
        args += ["--format", "{{.Name}}" if kind == "volume" else "{{.ID}}"]
        result[kind] = drill.docker("test-catalog-" + kind, *args).stdout.splitlines()
    return result


def emergency_teardown(drill):
    """The baseline is known broken. Do not release the slot with its resources.

    Resolve only this test's exact source name/label, validate every original
    bound before removal, and keep this recovery separate from primary proof.
    """
    if not (drill.containers or drill.volume_created or drill.network_created):
        return {"container": [], "volume": [], "network": []}
    record = drill.containers.get("source")
    if record is not None:
        found = drill.docker(
            "test-emergency-resolve", "container", "inspect", record["name"], check=False
        )
        if found.returncode == 0:
            values = json.loads(found.stdout)
            if len(values) != 1:
                raise RuntimeError("emergency container catalog cardinality mismatch")
            candidate = values[0]
            if candidate["Name"] != "/" + record["name"] or (
                candidate["Config"]["Labels"].get("deepref.task") != drill.label
            ):
                raise RuntimeError("emergency ownership mismatch; refuse deletion")
            record["id"] = pitr.bounded_identifier(candidate["Id"], r"[a-f0-9]{64}")
            drill.container("source")
            drill.docker("test-emergency-remove", "rm", "-fv", record["id"])
        elif owned_inventory(drill)["container"]:
            raise RuntimeError("container disposition remains uncertain")
    for kind, name, created in (
        ("volume", drill.volume_name, drill.volume_created),
        ("network", drill.network_name, drill.network_created),
    ):
        if not created:
            continue
        found = drill.docker("test-emergency-inspect-" + kind, kind, "inspect", name, check=False)
        if found.returncode == 0:
            value = drill.inspect(kind, name)
            if value["Name"] != name:
                raise RuntimeError("emergency resource identity mismatch")
            if kind == "network" and (not value.get("Internal") or value.get("Containers")):
                raise RuntimeError("emergency network is occupied or not internal")
            if kind == "volume" and value.get("Options") != {
                "type": "tmpfs", "device": "tmpfs",
                "o": f"size={pitr.ARCHIVE_BYTES},mode=0700",
            }:
                raise RuntimeError("emergency volume bounds mismatch")
            drill.docker("test-emergency-remove-" + kind, kind, "rm", name)
        elif owned_inventory(drill)[kind]:
            raise RuntimeError("resource disposition remains uncertain")
    remaining = owned_inventory(drill)
    if any(remaining.values()):
        raise RuntimeError("exact test-owned resources remain after emergency teardown")
    return remaining


@unittest.skipUnless(
    os.environ.get("DEEPREF_REQUIRE_PITR_CLEANUP") == "1",
    "requires an explicitly routed owned Docker fixture slot",
)
class InterruptedCreationCleanup(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.lock = open(pitr.FIXTURE_LOCK, "a+b")
        cls.signal_handlers = {}
        try:
            fcntl.flock(cls.lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            pitr.require_held_lock(pitr.FIXTURE_LOCK, cls.lock.fileno())
            pitr.guard_capacity()
            cls.output = Path(os.environ["DEEPREF_PITR_CLEANUP_EVIDENCE"])
            if cls.output.parent.resolve() != pitr.CACHE.resolve():
                raise ValueError("evidence must be a new exact child of the owned cache")
            cls.output.mkdir(mode=0o700)
            cls.pins = {
                str(p): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in (Path(pitr.__file__), Path(__file__))
            }
            (cls.output / "initial.json").write_text(json.dumps({
                "source": cls.pins,
                "disk_free_bytes": shutil.disk_usage(pitr.CACHE).free,
                "mem_available": next(line for line in Path("/proc/meminfo").read_text().splitlines()
                                      if line.startswith("MemAvailable:")),
                "scope": "real owned creation/cleanup only; no recovery proof",
            }, indent=2) + "\n")
            def interrupted(number, _frame):
                raise KeyboardInterrupt("owned cleanup regression interrupted")

            for number in (signal.SIGINT, signal.SIGTERM):
                cls.signal_handlers[number] = signal.signal(number, interrupted)
        except BaseException:
            for number, handler in cls.signal_handlers.items():
                signal.signal(number, handler)
            cls.lock.close()
            raise

    @classmethod
    def tearDownClass(cls):
        try:
            for name, expected in cls.pins.items():
                if hashlib.sha256(Path(name).read_bytes()).hexdigest() != expected:
                    raise RuntimeError("test source changed during owned fixture batch")
            (cls.output / "released.json").write_text(json.dumps({
                "source_unchanged": True, "disk_free_bytes": shutil.disk_usage(pitr.CACHE).free,
            }) + "\n")
        finally:
            for number, handler in cls.signal_handlers.items():
                signal.signal(number, handler)
            cls.lock.close()

    def exercise(self, cid_state):
        run = self.output / cid_state
        run.mkdir(mode=0o700)
        drill = InterruptedCreateDrill(run, uuid.uuid4().hex, cid_state)
        primary_clean = False
        primary_inventory = None
        try:
            pitr.guard_capacity()
            drill.create_resources()
            with self.assertRaises(LostCreateReply):
                drill.start("source")
            self.assertIsNotNone(drill.committed_id, "actual Docker creation did not complete")
            primary_clean = drill.cleanup()
            primary_inventory = owned_inventory(drill)
            (run / "primary-result.json").write_text(json.dumps({
                "cid_state": cid_state, "actual_created_id": drill.committed_id,
                "cleanup_returned": primary_clean, "remaining": primary_inventory,
                "result": copy.deepcopy(drill.result),
            }, indent=2) + "\n")
        finally:
            # Even semantic RED must leave no real fixture before releasing.
            # A second ordinary interruption must not strand baseline resources.
            handlers = {number: signal.signal(number, signal.SIG_IGN)
                        for number in (signal.SIGINT, signal.SIGTERM)}
            try:
                final_inventory = emergency_teardown(drill)
                (run / "independent-final-absence.json").write_text(
                    json.dumps(final_inventory, indent=2) + "\n"
                )
            finally:
                for number, handler in handlers.items():
                    signal.signal(number, handler)
        self.assertTrue(
            primary_clean and primary_inventory is not None and not any(primary_inventory.values()),
            "PITR_CID_CLEANUP_FAILED: interrupted create left owned resources before emergency teardown",
        )

    def test_empty_cid_is_resolved_and_removed(self):
        self.exercise("empty")

    def test_partial_cid_is_resolved_and_removed(self):
        self.exercise("partial")

    def test_non_ascii_cid_is_resolved_and_removed(self):
        self.exercise("non_ascii")

    def test_absent_cid_keeps_supported_resolution(self):
        self.exercise("absent")

    def test_valid_cid_keeps_supported_resolution(self):
        self.exercise("valid")
