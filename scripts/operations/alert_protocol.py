"""Local test envelope and one-shot receiver protocol; no delivered-page claim."""

from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import hmac
import json
import math
import re

from metrics_catalog import OWNER_ROLES

MAX_FRAME = 4096
TTL_SECONDS = 5
REQUEST_DEADLINE_SECONDS = 2
BINDING_FIELDS = {"rule_id", "delivery_test_id", "owner_role", "route", "catalog_sha256"}


def utc(value):
    if not isinstance(value, str) or not re.fullmatch(
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z", value
    ):
        raise ValueError("explicit UTC timestamp required")
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def hex_string(value, size):
    if not isinstance(value, str) or not re.fullmatch("[0-9a-f]{" + str(size) + "}", value):
        raise ValueError("invalid opaque identifier")
    return value


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON field")
        result[key] = value
    return result


def decode_frame(frame):
    if not isinstance(frame, bytes) or not 1 < len(frame) <= MAX_FRAME or not frame.endswith(b"\n") or b"\n" in frame[:-1]:
        raise ValueError("one bounded JSON line required")
    value = json.loads(frame.decode("utf-8"), object_pairs_hook=unique_fields,
                       parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite JSON")))
    if not isinstance(value, dict):
        raise ValueError("JSON object required")
    return value


def encode_frame(value):
    frame = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
                       allow_nan=False).encode("ascii") + b"\n"
    if len(frame) > MAX_FRAME:
        raise ValueError("oversized protocol frame")
    return frame


@dataclass(frozen=True)
class Binding:
    rule_id: str
    delivery_test_id: str
    owner_role: str
    route: str
    catalog_sha256: str

    @classmethod
    def from_validated_rule(cls, rule, catalog_sha256):
        """Caller supplies a row from validate_catalog, never a remote route."""
        rule_id = rule["id"]
        if not isinstance(rule_id, str) or not re.fullmatch(r"[a-z][a-z0-9-]{1,63}", rule_id):
            raise ValueError("bounded rule ID required")
        owner = rule["owner_role"]
        if not isinstance(owner, str) or owner not in OWNER_ROLES or rule["delivery_test_id"] != "local-" + rule_id + "-v1":
            raise ValueError("invalid local owner/test identity")
        return cls(rule_id, rule["delivery_test_id"], owner, "local-" + owner + "-v1",
                   hex_string(catalog_sha256, 64))

    def fields(self):
        return {name: getattr(self, name) for name in BINDING_FIELDS}


@dataclass(frozen=True)
class ValidatedRequest:
    binding: Binding
    nonce: str
    digest: str
    issued_at: datetime


def new_request(binding, *, nonce, token, issued_at):
    utc(issued_at)
    return {"schema_version": 1, "kind": "local_test", **binding.fields(),
            "nonce": hex_string(nonce, 32), "token": hex_string(token, 64),
            "issued_at": issued_at, "ttl_seconds": TTL_SECONDS}


def validate_request(frame, *, binding, nonce, token, now):
    row = decode_frame(frame)
    if set(row) != BINDING_FIELDS | {"schema_version", "kind", "nonce", "token", "issued_at", "ttl_seconds"}:
        raise ValueError("unsupported request fields")
    if type(row["schema_version"]) is not int or row["schema_version"] != 1 or row["kind"] != "local_test":
        raise ValueError("local-test schema required")
    if {name: row[name] for name in BINDING_FIELDS} != binding.fields():
        raise ValueError("request route/owner/rule/catalog mismatch")
    if row["nonce"] != hex_string(nonce, 32) or not hmac.compare_digest(hex_string(row["token"], 64), hex_string(token, 64)):
        raise ValueError("request identity mismatch")
    issued = utc(row["issued_at"])
    if type(row["ttl_seconds"]) is not int or row["ttl_seconds"] != TTL_SECONDS or not 0 <= (now - issued).total_seconds() <= TTL_SECONDS:
        raise ValueError("stale or future test request")
    return ValidatedRequest(binding, nonce, hashlib.sha256(encode_frame(row)).hexdigest(), issued)


def acknowledgement(request, *, accepted_at):
    accepted = utc(accepted_at)
    if not 0 <= (accepted - request.issued_at).total_seconds() <= TTL_SECONDS:
        raise ValueError("acknowledgement outside request validity")
    return {"schema_version": 1, "kind": "local_test_ack", **request.binding.fields(),
            "nonce": request.nonce, "request_sha256": request.digest, "accepted_at": accepted_at}


def verify_acknowledgement(frame, *, request, now, elapsed_seconds):
    if type(elapsed_seconds) not in (float, int) or not math.isfinite(elapsed_seconds) or not 0 <= elapsed_seconds <= REQUEST_DEADLINE_SECONDS:
        raise ValueError("request exceeded monotonic deadline")
    row = decode_frame(frame)
    if set(row) != BINDING_FIELDS | {"schema_version", "kind", "nonce", "request_sha256", "accepted_at"}:
        raise ValueError("unsupported acknowledgement fields")
    if type(row["schema_version"]) is not int or row["schema_version"] != 1 or row["kind"] != "local_test_ack":
        raise ValueError("local-test acknowledgement required")
    if {name: row[name] for name in BINDING_FIELDS} != request.binding.fields() or row["nonce"] != request.nonce or row["request_sha256"] != request.digest:
        raise ValueError("acknowledgement binding mismatch")
    accepted = utc(row["accepted_at"])
    if not request.issued_at <= accepted <= now or not 0 <= (now - request.issued_at).total_seconds() <= TTL_SECONDS:
        raise ValueError("stale or future acknowledgement")
    return {"schema_version": 1, "protocol_ack_validated": True,
            **request.binding.fields(), "nonce": request.nonce,
            "accepted_at": row["accepted_at"], "elapsed_seconds": elapsed_seconds,
            "wire_delivery_verified": False, "production_accepted": False,
            "backend_ingestion_verified": False, "human_notification_verified": False}


class OneShotReceiver:
    """Prebound local receiver; no sender-selected route or notification target."""

    def __init__(self, binding, *, nonce, token):
        self.binding = binding
        self.nonce = hex_string(nonce, 32)
        self._token = hex_string(token, 64)
        self._consumed = False

    def accept(self, frame, *, now):
        if self._consumed:
            raise ValueError("test nonce already consumed")
        request = validate_request(frame, binding=self.binding, nonce=self.nonce,
                                   token=self._token, now=now)
        response = encode_frame(acknowledgement(
            request, accepted_at=now.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")))
        # No await between validation and consumption: competing clients cannot
        # both consume the same nonce on the receiver's event loop.
        self._consumed = True
        return response
