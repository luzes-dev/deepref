"""Pure protocol tests; this suite never opens a socket or proves delivery."""

import importlib.util
import json
import os
from pathlib import Path
import sys
import unittest
from datetime import datetime, timedelta, timezone

REPOSITORY = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPOSITORY / "scripts/operations"))
SOURCE = Path(os.environ.get("DEEPREF_DELIVERY_SOURCE", str(REPOSITORY / "scripts/operations/alert_protocol.py")))
spec = importlib.util.spec_from_file_location("delivery_contract_draft", SOURCE)
protocol = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = protocol
spec.loader.exec_module(protocol)

NOW = datetime(2026, 10, 29, 4, 0, 1, tzinfo=timezone.utc)
NONCE = "1234567890abcdef1234567890abcdef"
TOKEN = "a" * 64
CATALOG_SHA = "b" * 64


class DeliveryContractTests(unittest.TestCase):
    def setUp(self):
        self.binding = protocol.Binding.from_validated_rule(
            {"id": "queue-age", "owner_role": "worker-owner", "delivery_test_id": "local-queue-age-v1"}, CATALOG_SHA)
        self.row = protocol.new_request(self.binding, nonce=NONCE, token=TOKEN,
                                        issued_at="2026-10-29T04:00:00Z")
        self.request = self.validate(self.row)
        self.ack = protocol.acknowledgement(self.request, accepted_at="2026-10-29T04:00:01Z")

    def validate(self, row, now=NOW):
        return protocol.validate_request(protocol.encode_frame(row), binding=self.binding,
                                         nonce=NONCE, token=TOKEN, now=now)

    def verify(self, row, *, now=NOW, elapsed=0.1):
        return protocol.verify_acknowledgement(protocol.encode_frame(row), request=self.request,
                                               now=now, elapsed_seconds=elapsed)

    def test_valid_ack_is_token_free_and_cannot_claim_delivery_or_production(self):
        receipt = self.verify(self.ack)
        self.assertEqual(receipt["route"], "local-worker-owner-v1")
        self.assertEqual(receipt["delivery_test_id"], "local-queue-age-v1")
        self.assertTrue(receipt["protocol_ack_validated"])
        for flag in ("wire_delivery_verified", "production_accepted", "backend_ingestion_verified", "human_notification_verified"):
            self.assertIs(receipt[flag], False)
        self.assertNotIn(TOKEN, json.dumps(receipt))
        self.assertNotIn("token", receipt)

    def test_ack_to_wrong_route_is_refused(self):
        self.ack["route"] = "local-api-owner-v1"
        with self.assertRaisesRegex(ValueError, "binding mismatch"):
            self.verify(self.ack)

    def test_rule_owner_test_catalog_nonce_and_request_digest_drift_are_refused(self):
        fields = {"rule_id": "other-rule", "owner_role": "api-owner", "delivery_test_id": "local-other-rule-v1",
                  "catalog_sha256": "c" * 64, "nonce": "d" * 32, "request_sha256": "e" * 64}
        for key, changed in fields.items():
            with self.subTest(field=key), self.assertRaisesRegex(ValueError, "binding mismatch"):
                self.verify({**self.ack, key: changed})

    def test_receiver_refuses_wrong_route_owner_test_and_credential(self):
        for key, changed in {"rule_id": "other-rule", "route": "local-api-owner-v1", "owner_role": "api-owner",
                             "delivery_test_id": "local-other-v1", "catalog_sha256": "c" * 64,
                             "nonce": "d" * 32, "token": "e" * 64}.items():
            with self.subTest(field=key), self.assertRaises(ValueError):
                self.validate({**self.row, key: changed})

    def test_stale_future_and_non_utc_requests_are_refused(self):
        for value in ("2026-10-29T03:59:55Z", "2026-10-29T04:00:02Z", "2026-10-29T04:00:00",
                      "2026-10-29T04:00:00+00:00"):
            with self.subTest(timestamp=value), self.assertRaises(ValueError):
                self.validate({**self.row, "issued_at": value})
        with self.assertRaises(ValueError):
            self.validate({**self.row, "ttl_seconds": True})

    def test_future_backdated_and_expired_acknowledgements_are_refused(self):
        for value in ("2026-10-29T04:00:02Z", "2026-10-29T03:59:59Z"):
            with self.subTest(timestamp=value), self.assertRaises(ValueError):
                self.verify({**self.ack, "accepted_at": value})
        with self.assertRaises(ValueError):
            self.verify(self.ack, now=NOW + timedelta(seconds=5))
        with self.assertRaises(ValueError):
            protocol.acknowledgement(self.request, accepted_at="2026-10-29T04:00:06Z")

    def test_wall_clock_cannot_replace_monotonic_request_deadline(self):
        for value in (2.001, -0.001, float("nan"), float("inf"), True):
            with self.subTest(elapsed=value), self.assertRaises(ValueError):
                self.verify(self.ack, elapsed=value)
        self.assertTrue(self.verify(self.ack, elapsed=2)["protocol_ack_validated"])

    def test_oversize_duplicate_extra_nonfinite_or_multiline_json_is_refused(self):
        for frame in (b"{}\n\n", b"x" * 4096 + b"\n", b'{"kind":1,"kind":2}\n', b'{"x":NaN}\n', b"[]\n", b"\xff\n"):
            with self.subTest(frame=frame[:40]), self.assertRaises((ValueError, UnicodeDecodeError)):
                protocol.decode_frame(frame)
        with self.assertRaises(ValueError):
            self.verify({**self.ack, "production_accepted": True})
        with self.assertRaises(ValueError):
            self.validate({**self.row, "tenant_id": "forbidden"})

    def test_receiver_accepts_a_nonce_only_once(self):
        receiver = protocol.OneShotReceiver(self.binding, nonce=NONCE, token=TOKEN)
        frame = protocol.encode_frame(self.row)
        self.assertTrue(self.verify(protocol.decode_frame(receiver.accept(frame, now=NOW)))["protocol_ack_validated"])
        with self.assertRaisesRegex(ValueError, "already consumed"):
            receiver.accept(frame, now=NOW)

    def test_wrong_route_cannot_consume_the_expected_nonce(self):
        receiver = protocol.OneShotReceiver(self.binding, nonce=NONCE, token=TOKEN)
        with self.assertRaisesRegex(ValueError, "mismatch"):
            receiver.accept(protocol.encode_frame({**self.row, "route": "local-api-owner-v1"}), now=NOW)
        ack = receiver.accept(protocol.encode_frame(self.row), now=NOW)
        self.assertTrue(self.verify(protocol.decode_frame(ack))["protocol_ack_validated"])


if __name__ == "__main__":
    unittest.main()
