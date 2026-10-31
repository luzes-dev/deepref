#!/usr/bin/env python3
"""Convert a bounded local OTLP JSON gauge capture into offline assessments.

Only the application's closed gauge profile is mapped. No endpoint is opened;
this file conversion does not verify SDK/gRPC or backend ingestion. JSON mapping:
https://opentelemetry.io/docs/specs/otlp/#json-protobuf-encoding
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re

from metrics_catalog import (
    CLOSED_DIMENSIONS, number, read_json_bounded, validate_catalog,
)

ROOT = Path(__file__).resolve().parents[2]
DB_TIME = "deepref.worker.metrics.last_success"
DB_UP = "deepref.worker.metrics.sample.up"
TRANSPORT_TIME = "deepref.transport.readiness.last_observed"
DB_UNITS = {
    "deepref.worker.queue.oldest_due.age": "s", "deepref.worker.dead.depth": "",
    "deepref.worker.live": "", "deepref.backlog.oldest.age": "s",
    "deepref.usage_export.identity_conflicts": "", "deepref.usage_export.unattributed": "",
    DB_TIME: "s", DB_UP: "",
}
TRANSPORT_UNITS = {"deepref.transport.ready": "", TRANSPORT_TIME: "s"}


def collection(value, maximum):
    if not isinstance(value, list) or len(value) > maximum:
        raise ValueError("unsupported capture collection size")
    return value


def integer(value, maximum, minimum=0):
    if isinstance(value, str) and re.fullmatch(r"-?[0-9]{1,20}", value):
        value = int(value)
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError("integer capture field out of range")
    return value


def string_value(value):
    if not isinstance(value, dict) or not isinstance(value.get("stringValue"), str):
        raise ValueError("string capture attribute required")
    known_types = {"stringValue", "boolValue", "intValue", "doubleValue", "arrayValue", "kvlistValue", "bytesValue"}
    if len(set(value) & known_types) != 1 or len(value["stringValue"]) > 128:
        raise ValueError("ambiguous or oversized capture attribute")
    return value["stringValue"]


def attributes(rows):
    values = {}
    for row in collection(rows, 64):
        if not isinstance(row, dict) or not isinstance(row.get("key"), str) or not 1 <= len(row["key"]) <= 128:
            raise ValueError("invalid capture attribute")
        if row["key"] in values:
            raise ValueError("duplicate capture attribute")
        values[row["key"]] = row.get("value")
    return values


def point(row, dimensions, now):
    if not isinstance(row, dict):
        raise ValueError("invalid gauge point")
    exported = integer(row.get("timeUnixNano"), 2**64 - 1, 1)
    if exported > int(now.timestamp() * 1_000_000_000):
        raise ValueError("future gauge export timestamp")
    # Missing optional protobuf flags mean zero; no measurement is defaulted.
    flags = integer(row.get("flags", 0), 2**32 - 1)
    if flags & ~1:
        raise ValueError("unsupported gauge flags")
    if integer(row.get("droppedAttributesCount", 0), 2**32 - 1) != 0:
        raise ValueError("incomplete gauge dimensions")
    attrs = attributes(row.get("attributes", []))
    if set(attrs) != dimensions:
        raise ValueError("gauge dimension scope mismatch")
    decoded = {key: string_value(value) for key, value in attrs.items()}
    if any(value not in CLOSED_DIMENSIONS[key] for key, value in decoded.items()):
        raise ValueError("unbounded gauge dimension")
    fields = set(row) & {"asInt", "asDouble"}
    if len(fields) > 1:
        raise ValueError("ambiguous gauge value")
    if flags == 1:
        return decoded, None
    if len(fields) != 1:
        raise ValueError("missing recorded gauge value")
    value = integer(row["asInt"], 2**63 - 1, -(2**63)) if "asInt" in fields else number(row["asDouble"])
    if value < 0:
        raise ValueError("negative operational gauge")
    return decoded, value


def source_timestamp(value):
    if value is None or value == 0:
        return None
    try:
        return datetime.fromtimestamp(value, timezone.utc).isoformat()
    except (ValueError, OverflowError, OSError) as error:
        raise ValueError("source capture timestamp out of range") from error


def convert_capture(document, catalog, *, service_name, now, repository=ROOT):
    validate_catalog(catalog, repository)
    if not isinstance(service_name, str) or not re.fullmatch(r"[a-zA-Z0-9_.-]{1,80}", service_name):
        raise ValueError("explicit bounded service name required")
    if not isinstance(document, dict):
        raise ValueError("OTLP capture object required")
    rows = [row for row in catalog["rules"] if row["source_kind"] in {"global_gauge", "process_gauge"}]
    bindings = {row["instruments"][0]: row for row in rows}
    for name, binding in bindings.items():
        kind = "global_gauge" if name in DB_UNITS else "process_gauge"
        if name not in DB_UNITS and name not in TRANSPORT_UNITS or binding["source_kind"] != kind:
            raise ValueError("catalog lacks an exact gauge capture binding")
    observations = {}
    processes = set()
    missing_joins = 0
    unbound = 0
    for resource in collection(document.get("resourceMetrics", []), 64):
        if not isinstance(resource, dict) or not isinstance(resource.get("resource"), dict):
            raise ValueError("capture resource required")
        attrs = attributes(resource["resource"].get("attributes", []))
        if "service.name" not in attrs or string_value(attrs["service.name"]) != service_name:
            unbound += 1
            continue
        if "service.instance.id" not in attrs:
            raise ValueError("selected resource lacks process identity")
        instance = string_value(attrs["service.instance.id"])
        if not re.fullmatch(r"[a-zA-Z0-9_-]{1,80}", instance):
            raise ValueError("bounded opaque process identity required")
        identity = hashlib.sha256((service_name + "\0" + instance).encode()).hexdigest()
        if identity in processes:
            raise ValueError("duplicate process capture")
        processes.add(identity)
        series = {}
        for scope in collection(resource.get("scopeMetrics", []), 16):
            if not isinstance(scope, dict) or not isinstance(scope.get("scope"), dict):
                raise ValueError("capture scope required")
            scope_name = scope["scope"].get("name")
            for metric in collection(scope.get("metrics", []), 64):
                if not isinstance(metric, dict):
                    raise ValueError("capture metric required")
                name = metric.get("name")
                if not isinstance(name, str):
                    raise ValueError("capture metric name required")
                if name not in DB_UNITS and name not in TRANSPORT_UNITS:
                    continue
                expected_scope = "deepref.worker" if name in DB_UNITS else "deepref.transport"
                expected_unit = (DB_UNITS if name in DB_UNITS else TRANSPORT_UNITS)[name]
                if scope_name != expected_scope or metric.get("unit", "") != expected_unit:
                    raise ValueError("gauge scope/unit mismatch")
                if set(metric) & {"gauge", "sum", "histogram", "exponentialHistogram", "summary"} != {"gauge"} or not isinstance(metric["gauge"], dict):
                    raise ValueError("expected gauge metric kind")
                dimensions = set(bindings[name]["dimensions"]) if name in bindings else set()
                for raw in collection(metric["gauge"].get("dataPoints", []), 128):
                    dims, value = point(raw, dimensions, now)
                    key = (name, tuple(sorted(dims.items())))
                    if key in series:
                        raise ValueError("duplicate selected gauge series")
                    series[key] = value
        for name, binding in bindings.items():
            timestamp_name = DB_TIME if name in DB_UNITS else TRANSPORT_TIME
            captured = source_timestamp(series.get((timestamp_name, ())))
            if captured is None:
                if any(key[0] == name for key in series):
                    missing_joins += 1
                continue
            # Transport has no DB sampler flag: its recorded readiness plus its
            # own observation timestamp form the capture, per the catalog policy.
            status = series.get((DB_UP, ())) if name in DB_UNITS else 1
            if status is not None and status not in {0, 1}:
                raise ValueError("invalid snapshot completeness gauge")
            for (instrument, dims), value in series.items():
                if instrument != name or value is None:
                    continue
                if binding["unit"] == "boolean" and value not in {0, 1}:
                    raise ValueError("invalid boolean gauge")
                panel = binding["id"] + "".join(":" + key + "=" + val for key, val in dims)
                observations.setdefault(panel, []).append({"instance": identity, "observed_at": captured,
                                                          "complete": status == 1, "dimensions": dict(dims), "value": value})
    snapshot = {"schema_version": 1, "observations": observations}
    canonical = json.dumps(document, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    receipt = {"profile": "deepref-otlp-gauges-v1", "capture_canonical_json_sha256": hashlib.sha256(canonical).hexdigest(),
               "selected_processes": len(processes), "missing_source_time_joins": missing_joins,
               "unbound_resources": unbound, "source_capture_adapter_verified": False,
               "backend_ingestion_verified": False, "production_accepted": False}
    return snapshot, receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--catalog", type=Path, default=ROOT / "operations/metrics/catalog.json")
    parser.add_argument("--service-name", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new local conversion directory")
    args = parser.parse_args()
    try:
        snapshot, receipt = convert_capture(read_json_bounded(args.capture), read_json_bounded(args.catalog),
                                            service_name=args.service_name, now=datetime.now(timezone.utc))
        encoded = json.dumps(snapshot, indent=2).encode()
        if len(encoded) > 262144:
            raise ValueError("normalized snapshot exceeds artifact limit")
        receipt["snapshot_sha256"] = hashlib.sha256(encoded).hexdigest()
        args.output.mkdir(mode=0o700)
        with (args.output / "snapshot.json").open("xb") as output:
            output.write(encoded)
        with (args.output / "provenance.json").open("x") as output:
            json.dump(receipt, output, indent=2)
            output.write("\n")
    except (ValueError, OSError, TypeError, RecursionError):
        parser.exit(2, "unsupported or incomplete local gauge capture; no verified ingestion\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
