"""Pure freshness/replica aggregation prototype tests, no sockets or fixtures."""

from datetime import datetime, timedelta, timezone
import importlib.util
import os
from pathlib import Path
import unittest
import sys

REPOSITORY = Path(__file__).resolve().parents[3]
SOURCE = REPOSITORY / "scripts/operations/metrics_catalog.py"
CATALOG = REPOSITORY / "operations/metrics/catalog.json"

path = Path(os.environ.get("DEEPREF_CATALOG_SOURCE", str(SOURCE)))
spec = importlib.util.spec_from_file_location("operations_catalog_draft", path)
catalog = importlib.util.module_from_spec(spec)
spec.loader.exec_module(catalog)

NOW = datetime(2026, 10, 28, 22, 0, tzinfo=timezone.utc)


def sample(instance, value, *, age=0, complete=True):
    return {"instance": instance, "value": value,
            "observed_at": (NOW - timedelta(seconds=age)).isoformat(),
            "complete": complete, "dimensions": {"pool": "documents"}}


def evaluate(samples):
    return catalog.fresh_gauge(samples, now=NOW, max_age_seconds=90,
                               reducer="max", dimensions={"pool": "documents"})


class FreshGaugeTests(unittest.TestCase):
    def test_replicated_global_database_samples_are_never_summed(self):
        result = evaluate([sample("worker-a", 7), sample("worker-b", 7)])
        self.assertEqual(result, {"state": "observed", "value": 7,
                                  "fresh_replicas": 2, "rejected_replicas": 0})

    def test_empty_failed_stale_future_are_unknown_instead_of_zero(self):
        for rows in [[], [sample("failed", 0, complete=False)],
                     [sample("stale", 0, age=91)], [sample("future", 0, age=-1)]]:
            with self.subTest(rows=rows):
                result = evaluate(rows)
                self.assertEqual(result["state"], "unknown")
                self.assertIsNone(result["value"])
                self.assertEqual(result["fresh_replicas"], 0)

    def test_fresh_zero_is_distinct_from_missing_and_rejected_coverage_is_visible(self):
        result = evaluate([sample("fresh", 0, age=90), sample("failed", 100, complete=False)])
        self.assertEqual(result, {"state": "observed", "value": 0,
                                  "fresh_replicas": 1, "rejected_replicas": 1})

    def test_invalid_or_duplicate_process_samples_fail_closed(self):
        invalid = [sample("invalid", float("nan")), sample("invalid", True),
                   {**sample("invalid", 1), "observed_at": "2026-10-28T22:00:00"},
                   {**sample("invalid", 1), "dimensions": {"workspace_id": "secret"}},
                   {**sample("invalid", 1), "raw_error": "secret"}]
        for row in invalid:
            with self.subTest(row=row), self.assertRaises(ValueError):
                evaluate([row])
        with self.assertRaises(ValueError):
            evaluate([sample("same", 1), sample("same", 2)])

    def test_global_population_cannot_switch_to_sum_or_unbounded_age(self):
        for reducer, age in [("sum", 90), ("max", 0), ("max", 3601), ("max", True)]:
            with self.subTest(reducer=reducer, age=age), self.assertRaises(ValueError):
                catalog.fresh_gauge([], now=NOW, max_age_seconds=age,
                                    reducer=reducer, dimensions={})


class CatalogValidationTests(unittest.TestCase):
    def setUp(self):
        import json
        self.value = json.loads(CATALOG.read_text())
        self.repository = REPOSITORY

    def validate(self):
        return catalog.validate_catalog(self.value, self.repository)

    def test_actual_catalog_has_real_runbook_headings_and_never_activates(self):
        result = self.validate()
        self.assertEqual(result["rules"], 14)
        self.assertEqual(result["missing_source_rows"], 3)
        self.assertFalse(result["production_activation_permitted"])
        self.assertFalse(result["verified_ingestion"])
        self.assertFalse(result["real_notification_delivery"])

    def test_unbounded_metric_labels_and_invented_sources_are_refused(self):
        import copy
        original = copy.deepcopy(self.value)
        self.value["rules"][3]["dimensions"] = {"workspace_id": "private"}
        with self.assertRaises(ValueError):
            self.validate()
        self.value = original
        self.value["rules"][3]["instruments"] = ["deepref.object.errors"]
        with self.assertRaises(ValueError):
            self.validate()

    def test_prototype_cannot_claim_approved_thresholds_or_production_activation(self):
        self.value["production_activation_permitted"] = True
        with self.assertRaises(ValueError):
            self.validate()
        self.value["production_activation_permitted"] = False
        self.value["rules"][0]["thresholds_approved"] = True
        with self.assertRaises(ValueError):
            self.validate()

    def test_global_gauge_sum_and_wrong_instrument_unit_are_refused(self):
        self.value["rules"][3]["replica_reducer"] = "sum_rates"
        with self.assertRaises(ValueError):
            self.validate()
        self.value["rules"][3]["replica_reducer"] = "max"
        self.value["rules"][3]["unit"] = "jobs"
        with self.assertRaises(ValueError):
            self.validate()

    def test_missing_runbook_heading_and_delivery_identity_are_refused(self):
        original = self.value["rules"][0]["runbook"]
        self.value["rules"][0]["runbook"] = "docs/metrics-and-alerts-runbook.md#missing-heading"
        with self.assertRaises(ValueError):
            self.validate()
        self.value["rules"][0]["runbook"] = original
        self.value["rules"][0]["delivery_test_id"] = ""
        with self.assertRaises(ValueError):
            self.validate()

    def test_missing_source_and_query_binding_cannot_silently_become_numeric(self):
        self.value["rules"][11]["instruments"] = ["deepref.worker.live"]
        with self.assertRaises(ValueError):
            self.validate()
        self.value["rules"][11]["instruments"] = []
        self.value["rules"][0]["source_gap"] = ""
        with self.assertRaises(ValueError):
            self.validate()


class DashboardPrototypeTests(unittest.TestCase):
    def setUp(self):
        import json
        self.value = json.loads(CATALOG.read_text())
        self.repository = REPOSITORY

    def test_missing_queries_and_external_sources_stay_unknown_with_numeric_input(self):
        snapshot = {"schema_version": 1, "observations": {"api-availability": [sample("forged", 1)]}}
        panels = catalog.assess_panels(self.value, snapshot, NOW)
        panel = next(p for p in panels if p["id"] == "api-availability")
        self.assertEqual(panel["state"], "unknown")
        self.assertIsNone(panel["value"])
        self.assertIsNone(panel["proposal"])

    def test_fresh_gauge_keeps_capture_expiry_and_replica_max_without_summing(self):
        panel_id = "queue-age:pool=documents"
        snapshot = {"schema_version": 1, "observations": {panel_id: [sample("a", 70), sample("b", 70, age=5)]}}
        panel = next(p for p in catalog.assess_panels(self.value, snapshot, NOW) if p["id"] == panel_id)
        self.assertEqual(panel["value"], 70)
        self.assertEqual(panel["proposal"], "ticket")
        self.assertEqual(panel["expires_at_ms"], (NOW.timestamp() + 85) * 1000)

    def test_unknown_panel_and_negative_gauge_refuse_instead_of_healthy(self):
        with self.assertRaises(ValueError):
            catalog.assess_panels(self.value, {"schema_version": 1, "observations": {"tenant-key": []}}, NOW)
        with self.assertRaises(ValueError):
            catalog.assess_panels(self.value, {"schema_version": 1, "observations": {"queue-age:pool=documents": [sample("a", -1)]}}, NOW)

    def test_dashboard_escapes_catalog_text_and_has_no_background_fetch(self):
        self.value["rules"][0]["title"] = "<script>PRIVATE</script>"
        html = catalog.render_html(self.value, {"schema_version": 1, "observations": {}}, self.repository, NOW)
        self.assertNotIn("<script>PRIVATE", html)
        self.assertIn("&lt;script&gt;PRIVATE&lt;/script&gt;", html)
        self.assertIn("Production activation is disabled", html)
        self.assertIn("expireStaleObservations", html)
        self.assertNotIn("fetch(", html)


class CapturePolicyTests(unittest.TestCase):
    def test_metric_export_and_heartbeat_cannot_rejuvenate_capture_time(self):
        import copy
        import json
        value = json.loads(CATALOG.read_text())
        repository = REPOSITORY
        for substitute in ["metric_export_time", "heartbeat_seen_at"]:
            changed = copy.deepcopy(value)
            changed["rules"][3]["capture_policy"]["observed_at"] = substitute
            with self.subTest(substitute=substitute), self.assertRaises(ValueError):
                catalog.validate_catalog(changed, repository)


class JsonInputTests(unittest.TestCase):
    def test_duplicate_json_fields_are_refused_instead_of_selecting_last_value(self):
        import tempfile
        with tempfile.TemporaryDirectory(prefix="deepref-catalog-json-") as root:
            path = Path(root) / "duplicate.json"
            path.write_text('{"schema_version":1,"schema_version":2}')
            with self.assertRaisesRegex(ValueError, "duplicate"):
                catalog.read_json_bounded(path)

    def test_nonfinite_symlink_and_nonregular_inputs_are_refused(self):
        import tempfile
        with tempfile.TemporaryDirectory(prefix="deepref-catalog-input-") as root:
            root = Path(root)
            path = root / "input.json"
            path.write_text('{"value":NaN}')
            with self.assertRaises(ValueError):
                catalog.read_json_bounded(path)
            link = root / "link.json"
            link.symlink_to(path)
            with self.assertRaises(OSError):
                catalog.read_json_bounded(link)
            os.mkfifo(root / "fifo")
            with self.assertRaisesRegex(ValueError, "regular JSON"):
                catalog.read_json_bounded(root / "fifo")

    def test_cli_emits_capture_decisions_without_claiming_ingestion_or_window(self):
        import json
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory(prefix="deepref-catalog-cli-") as root:
            root = Path(root)
            snapshot = root / "snapshot.json"
            snapshot.write_text(json.dumps({"schema_version": 1, "observations": {
                "queue-age:pool=documents": [{"instance": "fixture-a", "observed_at": datetime.now(timezone.utc).isoformat(),
                                              "complete": True, "dimensions": {"pool": "documents"}, "value": 70}]}}))
            output = root / "rendered"
            command = [sys.executable, str(SOURCE), "--catalog", str(CATALOG), "--repository", str(REPOSITORY),
                       "--snapshot", str(snapshot), "--output", str(output)]
            result = subprocess.run(command, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            assessment = json.loads((output / "assessment.json").read_text())
            self.assertFalse(assessment["source_capture_adapter_verified"])
            self.assertFalse(assessment["sustained_window_verified"])
            self.assertFalse(assessment["production_proof"])
            self.assertEqual(assessment["unknown_panels"], 38)
            panel = next(row for row in assessment["panel_assessments"] if row["id"] == "queue-age:pool=documents")
            self.assertEqual((panel["state"], panel["value"], panel["proposal"]), ("observed", 70, "ticket"))
            self.assertEqual(subprocess.run(command + ["--production"], capture_output=True, timeout=5).returncode, 2)
            before = (output / "assessment.json").read_bytes()
            self.assertEqual(subprocess.run(command, capture_output=True, timeout=5).returncode, 2)
            self.assertEqual((output / "assessment.json").read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
