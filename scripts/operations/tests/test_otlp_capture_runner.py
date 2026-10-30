"""Pure capture-evidence refusal tests; no SDK, socket, Cargo or lock proof."""

import copy
from datetime import timedelta
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/operations"))
import test_otlp_capture as runner
from test_otlp_snapshot import NOW, adapter, attr, gauge, resource


def phase_files(root):
    """Modeled file set, explicitly distinct from actual RPC capture evidence."""
    for phase_index, phase in enumerate(("stale", "fresh", "failed")):
        rows = []
        names = []
        for index in range(2):
            row = resource("fixture-" + str(index),
                           captured=(NOW - timedelta(seconds=91 if phase == "stale" else 2)).timestamp(),
                           complete=0 if phase == "failed" else 1)
            row["resource"]["attributes"][0] = attr("service.name", runner.SERVICE)
            row["scopeMetrics"].append({"scope": {"name": "deepref.transport"}, "metrics": [
                gauge("deepref.transport.ready", 0 if phase == "failed" else 1),
                gauge(adapter.TRANSPORT_TIME, (NOW - timedelta(seconds=1)).timestamp(), unit="s")]})
            rows.append(row)
            name = "received-" + str(phase_index * 2 + index + 1).zfill(2)
            names.append(name)
            (root / (name + ".json")).write_text(json.dumps({"resourceMetrics": [row]}))
            # Pure fixtures never claim these bytes are a real protobuf.
            (root / (name + ".reencoded.pb")).write_bytes(b"synthetic-non-protobuf")
        (root / ("phase-" + phase + ".json")).write_text(json.dumps({"resourceMetrics": rows}))
        (root / ("phase-" + phase + "-sources.json")).write_text(json.dumps({
            "assembled_from_received_rpc_files": names, "original_network_frames_retained": False,
            "gRPC_fixture_capture": True, "backend_ingestion_verified": False, "production_accepted": False}))


class CaptureRunnerEvidenceTests(unittest.TestCase):
    def test_modeled_assembly_assesses_states_without_upgrading_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            phase_files(root)
            result = runner.phase_assertions(root, now=NOW)
            self.assertEqual([result[name]["queue"]["state"] for name in ("stale", "fresh", "failed")],
                             ["unknown", "observed", "unknown"])
            self.assertEqual(result["fresh"]["queue"]["value"], 7)
            for phase in result.values():
                self.assertFalse(phase["file_provenance"]["source_capture_adapter_verified"])
                self.assertFalse(phase["file_provenance"]["backend_ingestion_verified"])
                self.assertEqual(len(phase["received_artifacts"]), 4)

    def test_combined_phase_edit_cannot_impersonate_retained_received_data(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            phase_files(root)
            path = root / "phase-fresh.json"
            body = json.loads(path.read_text())
            # The other replica still reports 7, so max remains 7. Aggregate
            # assertions cannot detect this provenance violation themselves.
            body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][0]["asDouble"] = 5.0
            path.write_text(json.dumps(body))
            with self.assertRaisesRegex(ValueError, "differs from its received RPCs"):
                runner.phase_assertions(root, now=NOW)

    def test_source_pointer_replay_and_traversal_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            phase_files(root)
            path = root / "phase-fresh-sources.json"
            original = json.loads(path.read_text())
            for names in (["received-03", "received-03"], ["../received-03", "received-04"], ["received-03"]):
                value = copy.deepcopy(original)
                value["assembled_from_received_rpc_files"] = names
                path.write_text(json.dumps(value))
                with self.subTest(names=names), self.assertRaisesRegex(ValueError, "exact received-RPC"):
                    runner.received_phase(root, "fresh")

    def test_duplicate_resources_fail_even_with_matching_combined_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            phase_files(root)
            request = json.loads((root / "received-03.json").read_text())
            (root / "received-04.json").write_text(json.dumps(request))
            (root / "phase-fresh.json").write_text(json.dumps({"resourceMetrics": request["resourceMetrics"] * 2}))
            with self.assertRaisesRegex(ValueError, "duplicate process"):
                runner.phase_assertions(root, now=NOW)

    def test_missing_protobuf_and_false_capture_boundaries_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            phase_files(root)
            (root / "received-03.reencoded.pb").unlink()
            with self.assertRaisesRegex(ValueError, "retained file bounds"):
                runner.received_phase(root, "fresh")
            sidecar_path = root / "phase-stale-sources.json"
            original = json.loads(sidecar_path.read_text())
            for key in ("original_network_frames_retained", "backend_ingestion_verified", "production_accepted"):
                value = {**original, key: True}
                sidecar_path.write_text(json.dumps(value))
                with self.subTest(key=key), self.assertRaisesRegex(ValueError, "exact received-RPC"):
                    runner.received_phase(root, "stale")

    def test_both_numeric_resource_guards_are_enforced_at_boundary(self):
        minimum = {"minimum_free_bytes": 4294967296, "minimum_mem_available_bytes": 6442450944,
                   "free_bytes": 4294967296, "mem_available_bytes": 6442450944}
        runner.require_guards(minimum)
        for key in ("free_bytes", "mem_available_bytes"):
            with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, "guard refused"):
                runner.require_guards({**minimum, key: minimum[key] - 1})

    def test_process_refusal_does_not_suppress_independent_cleanup_observations(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "fixture"
            fixture.mkdir()
            (root / "tmp").mkdir()
            (fixture / "owned-port.json").write_text(json.dumps({
                "test": runner.CASE, "scope": "local_metric_fixture", "port": 12345}))
            result = {"exit": 1}
            modeled_pins = {"files": {"synthetic": "fixture"}}
            with (patch.object(runner, "owned_process", side_effect=ValueError("modeled identity refusal")),
                  patch.object(runner, "owned_closed", return_value=True) as closed,
                  patch.object(runner, "pins", return_value=modeled_pins),
                  patch.object(runner, "guards", return_value={"synthetic": True})):
                runner.final_inventory(root, "modeled", modeled_pins, result)
            closed.assert_called_once_with(12345)  # Mock only; no real connect.
            self.assertEqual(result["cleanup_exit"], 1)
            self.assertEqual([row["step"] for row in result["cleanup_errors"]], ["process"])
            self.assertEqual(result["tmp_entries"], [])
            self.assertEqual(result["source_after"], modeled_pins)
            self.assertEqual(result["final_guards"], {"synthetic": True})


if __name__ == "__main__":
    unittest.main()
