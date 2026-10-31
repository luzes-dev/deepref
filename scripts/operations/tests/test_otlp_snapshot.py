"""Synthetic OTLP-shaped capture tests, not actual exporter ingestion proof."""

import copy
from datetime import datetime, timedelta, timezone
import importlib.util
import json
import os
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/operations"))
import metrics_catalog

SOURCE = Path(os.environ.get("DEEPREF_OTLP_SOURCE", str(ROOT / "scripts/operations/otlp_snapshot.py")))
spec = importlib.util.spec_from_file_location("otlp_snapshot_under_test", SOURCE)
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)
CATALOG = json.loads((ROOT / "operations/metrics/catalog.json").read_text())
NOW = datetime(2026, 10, 29, 12, 0, tzinfo=timezone.utc)


def attr(key, value):
    return {"key": key, "value": {"stringValue": value}}


def gauge(name, value, *, dimensions=None, unit="", flags=0, exported_at=NOW):
    point = {"timeUnixNano": str(int(exported_at.timestamp()) * 1_000_000_000), "flags": flags,
             "attributes": [attr(k, v) for k, v in (dimensions or {}).items()]}
    point["asInt" if type(value) is int else "asDouble"] = str(value) if type(value) is int else value
    return {"name": name, "unit": unit, "gauge": {"dataPoints": [point]}}


def resource(instance="fixture-a", *, captured=None, complete=1, exported_at=NOW):
    captured = (exported_at - timedelta(seconds=2)).timestamp() if captured is None else captured
    return {"resource": {"attributes": [attr("service.name", "fixture-worker"), attr("service.instance.id", instance)]},
            "scopeMetrics": [{"scope": {"name": "deepref.worker"}, "metrics": [
                gauge("deepref.worker.queue.oldest_due.age", 7.0, dimensions={"pool": "documents"}, unit="s", exported_at=exported_at),
                gauge(adapter.DB_TIME, captured, unit="s", exported_at=exported_at),
                gauge(adapter.DB_UP, complete, exported_at=exported_at)]}]}


def normalize(rows):
    return adapter.convert_capture({"resourceMetrics": rows}, CATALOG, service_name="fixture-worker", now=NOW, repository=ROOT)


def queue(snapshot):
    return next(row for row in metrics_catalog.assess_panels(CATALOG, snapshot, NOW) if row["id"] == "queue-age:pool=documents")


class CaptureGaugeTests(unittest.TestCase):
    def test_two_processes_reporting_global_seven_assess_seven_and_keep_unverified(self):
        snapshot, receipt = normalize([resource("fixture-a"), resource("fixture-b")])
        panel = queue(snapshot)
        self.assertEqual((panel["state"], panel["value"], panel["fresh_replicas"]), ("observed", 7, 2))
        self.assertEqual(receipt["selected_processes"], 2)
        for flag in ("source_capture_adapter_verified", "backend_ingestion_verified", "production_accepted"):
            self.assertIs(receipt[flag], False)
        self.assertNotIn("fixture-a", json.dumps(snapshot))

    def test_fresh_export_does_not_rejuvenate_stale_source_capture(self):
        captured = NOW - timedelta(seconds=91)
        snapshot, _ = normalize([resource(captured=captured.timestamp())])
        self.assertEqual(snapshot["observations"]["queue-age:pool=documents"][0]["observed_at"], captured.isoformat())
        self.assertEqual(queue(snapshot)["state"], "unknown")
        self.assertIsNone(queue(snapshot)["value"])

    def test_failed_missing_and_future_completeness_never_invent_zero_or_freshness(self):
        for row in (resource(complete=0), resource(captured=(NOW + timedelta(seconds=1)).timestamp())):
            with self.subTest(row=row):
                snapshot, _ = normalize([row])
                self.assertEqual(queue(snapshot)["state"], "unknown")
                self.assertIsNone(queue(snapshot)["value"])
        row = resource()
        row["scopeMetrics"][0]["metrics"].pop()  # no sample.up
        snapshot, _ = normalize([row])
        self.assertEqual(queue(snapshot)["state"], "unknown")

    def test_missing_source_time_and_no_recorded_value_leave_no_measurement(self):
        row = resource()
        row["scopeMetrics"][0]["metrics"].pop(1)
        snapshot, receipt = normalize([row])
        self.assertEqual(snapshot["observations"], {})
        self.assertEqual(receipt["missing_source_time_joins"], 1)
        row = resource()
        row["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][0]["flags"] = 1
        snapshot, _ = normalize([row])
        self.assertEqual(snapshot["observations"], {})

    def test_wrong_scope_unit_dimensions_and_duplicate_process_or_series_refuse(self):
        for mutate in (
            lambda row: row["scopeMetrics"][0]["scope"].update(name="deepref.wrong"),
            lambda row: row["scopeMetrics"][0]["metrics"][0].update(unit="ms"),
            lambda row: row["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][0]["attributes"].append(attr("workspace_id", "forbidden")),
            lambda row: row["scopeMetrics"][0]["metrics"].append(copy.deepcopy(row["scopeMetrics"][0]["metrics"][0])),
        ):
            row = resource()
            mutate(row)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                normalize([row])
        with self.assertRaisesRegex(ValueError, "duplicate process"):
            normalize([resource(), resource()])

    def test_unknown_metadata_is_ignored_but_future_export_and_nonfinite_values_refuse(self):
        row = resource()
        row["futureMetadata"] = {"ignored": True}
        self.assertEqual(queue(normalize([row])[0])["value"], 7)
        for change in ({"asDouble": float("nan")}, {"timeUnixNano": str((int(NOW.timestamp()) + 1) * 1_000_000_000)}):
            row = resource()
            row["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][0].update(change)
            with self.subTest(change=change), self.assertRaises(ValueError):
                normalize([row])

    def test_transport_uses_its_own_capture_value_without_database_helpers(self):
        row = resource()
        row["scopeMetrics"] = [{"scope": {"name": "deepref.transport"}, "metrics": [
            gauge("deepref.transport.ready", 1),
            gauge(adapter.TRANSPORT_TIME, (NOW - timedelta(seconds=61)).timestamp(), unit="s")]}]
        snapshot, _ = normalize([row])
        panel = next(p for p in metrics_catalog.assess_panels(CATALOG, snapshot, NOW) if p["id"] == "transport-observation")
        self.assertEqual(panel["state"], "unknown")
        self.assertIsNone(panel["value"])

    def test_file_cli_can_feed_existing_dashboard_with_unverified_provenance(self):
        import hashlib
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory(prefix="deepref-otlp-file-") as root:
            root = Path(root)
            capture = root / "synthetic-capture.json"
            capture.write_text(json.dumps({"resourceMetrics": [resource(exported_at=datetime.now(timezone.utc) - timedelta(seconds=1))]}))
            output = root / "converted"
            command = [sys.executable, str(SOURCE), "--capture", str(capture), "--service-name", "fixture-worker", "--output", str(output)]
            result = subprocess.run(command, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            receipt = json.loads((output / "provenance.json").read_text())
            self.assertFalse(receipt["source_capture_adapter_verified"])
            self.assertFalse(receipt["backend_ingestion_verified"])
            self.assertFalse(receipt["production_accepted"])
            self.assertEqual(receipt["snapshot_sha256"], hashlib.sha256((output / "snapshot.json").read_bytes()).hexdigest())
            rendered = root / "rendered"
            result = subprocess.run([sys.executable, str(ROOT / "scripts/operations/metrics_catalog.py"), "--catalog", str(ROOT / "operations/metrics/catalog.json"),
                                     "--repository", str(ROOT), "--snapshot", str(output / "snapshot.json"), "--output", str(rendered)], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            assessment = json.loads((rendered / "assessment.json").read_text())
            panel = next(row for row in assessment["panel_assessments"] if row["id"] == "queue-age:pool=documents")
            self.assertEqual((panel["state"], panel["value"]), ("observed", 7))
            self.assertEqual(assessment["unknown_panels"], 38)
            self.assertFalse(assessment["source_capture_adapter_verified"])
            self.assertEqual(subprocess.run(command, capture_output=True, timeout=5).returncode, 2)


if __name__ == "__main__":
    unittest.main()
