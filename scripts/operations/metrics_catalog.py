#!/usr/bin/env python3
"""Offline operations catalog, fresh gauges and proposed rules; no deployed acceptance."""

from datetime import datetime, timezone
import math
import re

SCHEMA = 1
CLOSED_DIMENSIONS = {
    "pool": {"ai", "documents", "workflow", "integration", "projection"},
    "family": {"outbox", "autonomy_outbox", "inbox", "usage_settlement", "usage_export", "usage_unemitted", "object_cleanup", "retired_objects"},
    "route_class": {"health", "identity", "webhook", "billing", "account", "documents", "import", "assistant", "research", "other", "unmatched"},
}


def timestamp(value):
    if not isinstance(value, str) or len(value) > 40:
        raise ValueError("UTC source-capture timestamp required")
    result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if result.utcoffset() is None or result.utcoffset().total_seconds() != 0:
        raise ValueError("UTC timestamp required")
    return result


def number(value):
    if type(value) not in (int, float) or not math.isfinite(value):
        raise ValueError("finite numeric measurement required")
    return value


def fresh_gauge(samples, *, now, max_age_seconds, reducer, dimensions):
    """Pure gauge seam only; never pretends a counter snapshot is an SLI.

    Replica identity is supplied by a separately verified source adapter. A
    failed sample does not replace the previous value or refresh capture time.
    Unknown is explicit if no complete fresh source exists. A fresh source can
    coexist with a failed replica; separate sampler coverage alert stays needed.
    """
    if reducer not in {"max", "min"}:
        raise ValueError("global gauge reducer must be max or min")
    if type(max_age_seconds) is not int or not 1 <= max_age_seconds <= 3600:
        raise ValueError("bounded freshness interval required")
    expected = dict(dimensions)
    for key, value in expected.items():
        if key not in CLOSED_DIMENSIONS or value not in CLOSED_DIMENSIONS[key]:
            raise ValueError("unknown or unbounded dimension")
    if not isinstance(samples, list) or len(samples) > 1000:
        raise ValueError("bounded source list required")
    values = []
    replicas = set()
    rejected = 0
    for row in samples:
        if not isinstance(row, dict) or set(row) != {"instance", "observed_at", "complete", "dimensions", "value"}:
            raise ValueError("unsupported normalized sample")
        instance = row["instance"]
        if not isinstance(instance, str) or not re.fullmatch(r"[a-zA-Z0-9_-]{1,80}", instance):
            raise ValueError("bounded opaque process identity required")
        if instance in replicas:
            raise ValueError("duplicate process sample")
        replicas.add(instance)
        if row["dimensions"] != expected or type(row["complete"]) is not bool:
            raise ValueError("sample scope/completeness mismatch")
        value = number(row["value"])
        captured = timestamp(row["observed_at"])
        age = (now - captured).total_seconds()
        if not row["complete"] or age < 0 or age > max_age_seconds:
            rejected += 1
            continue
        values.append(value)
    if not values:
        return {"state": "unknown", "value": None, "fresh_replicas": 0, "rejected_replicas": rejected}
    return {"state": "observed", "value": (max if reducer == "max" else min)(values),
            "fresh_replicas": len(values), "rejected_replicas": rejected}


KNOWN_INSTRUMENTS = {
    "deepref.http.requests": "requests",
    "deepref.http.response_header.duration": "seconds",
    "deepref.schema.behind": "boolean",
    "deepref.worker.queue.oldest_due.age": "seconds",
    "deepref.worker.dead.depth": "jobs",
    "deepref.worker.live": "processes",
    "deepref.backlog.oldest.age": "seconds",
    "deepref.usage_export.identity_conflicts": "facts",
    "deepref.usage_export.unattributed": "facts",
    "deepref.transport.ready": "boolean",
    "deepref.worker.metrics.last_success": "unix_seconds",
    "deepref.worker.metrics.sample.up": "boolean",
}
INSTRUMENT_DIMENSIONS = {name: set() for name in KNOWN_INSTRUMENTS}
INSTRUMENT_DIMENSIONS.update({
    "deepref.http.requests": {"route_class"},
    "deepref.http.response_header.duration": {"route_class"},
    "deepref.worker.queue.oldest_due.age": {"pool"},
    "deepref.worker.dead.depth": {"pool"},
    "deepref.backlog.oldest.age": {"family"},
})
OWNER_ROLES = {"api-owner", "worker-owner", "platform-owner", "billing-owner", "ai-owner"}
RULE_FIELDS = {
    "id", "title", "source_kind", "instruments", "unit", "dimensions", "replica_reducer",
    "max_age_seconds", "ticket", "page", "comparison", "window_seconds", "owner_role",
    "runbook", "delivery_test_id", "thresholds_approved", "freshness", "source_gap", "capture_policy",
}


CAPTURE_POLICIES = {'db_snapshot': {'complete': 'deepref.worker.metrics.sample.up', 'observed_at': 'deepref.worker.metrics.last_success', 'timestamp_source': 'gauge_value'}, 'transport_observation': {'complete': None, 'observed_at': 'deepref.transport.readiness.last_observed', 'timestamp_source': 'gauge_value'}, 'query_capture': {'complete': 'verified_query_receipt', 'observed_at': 'source_window_end', 'timestamp_source': 'verified_query_binding'}, 'external_capture': {'complete': 'verified_catalog_receipt', 'observed_at': 'source_catalog_cutoff', 'timestamp_source': 'verified_external_binding'}}


def heading_slug(heading):
    return re.sub(r"[^a-z0-9\s-]", "", heading.lower()).strip().replace(" ", "-")


def validate_catalog(value, repository):
    if not isinstance(value, dict) or set(value) != {
        "schema_version", "status", "production_activation_permitted", "rules"
    }:
        raise ValueError("unsupported catalog shape")
    if type(value["schema_version"]) is not int or value["schema_version"] != SCHEMA:
        raise ValueError("unsupported catalog schema")
    if value["status"] != "local_proposals_unapproved" or value["production_activation_permitted"] is not False:
        raise ValueError("local catalog cannot activate production rules")
    rows = value["rules"]
    if not isinstance(rows, list) or not 1 <= len(rows) <= 100:
        raise ValueError("bounded nonempty rule catalog required")
    seen = set()
    for row in rows:
        if not isinstance(row, dict) or set(row) != RULE_FIELDS:
            raise ValueError("unsupported rule fields")
        rule_id = row["id"]
        if not isinstance(rule_id, str) or not re.fullmatch(r"[a-z][a-z0-9-]{1,63}", rule_id) or rule_id in seen:
            raise ValueError("unique bounded rule ID required")
        seen.add(rule_id)
        if not isinstance(row["title"], str) or not 1 <= len(row["title"]) <= 160:
            raise ValueError("bounded rule title required")
        if not isinstance(row["owner_role"], str) or row["owner_role"] not in OWNER_ROLES or row["delivery_test_id"] != "local-" + rule_id + "-v1":
            raise ValueError("owner/delivery identity missing")
        if row["thresholds_approved"] is not False:
            raise ValueError("local thresholds remain unapproved")
        if not isinstance(row["comparison"], str) or row["comparison"] not in {"above", "below"}:
            raise ValueError("unsupported threshold comparator")
        ticket, page = number(row["ticket"]), number(row["page"])
        if ticket < 0 or page < 0 or (row["comparison"] == "above" and page < ticket) or (row["comparison"] == "below" and page > ticket):
            raise ValueError("page must be at least as severe as ticket")
        if type(row["max_age_seconds"]) is not int or not 1 <= row["max_age_seconds"] <= 3600:
            raise ValueError("bounded freshness required")
        if type(row["window_seconds"]) is not int or not 1 <= row["window_seconds"] <= 86400:
            raise ValueError("bounded rule window required")
        dims = row["dimensions"]
        if not isinstance(dims, dict) or any(k not in CLOSED_DIMENSIONS or not isinstance(v, str) or v != "*" and v not in CLOSED_DIMENSIONS[k] for k, v in dims.items()):
            raise ValueError("unbounded/unknown metric dimension")
        instruments = row["instruments"]
        if not isinstance(instruments, list) or len(instruments) > 4 or any(not isinstance(name, str) or name not in KNOWN_INSTRUMENTS for name in instruments):
            raise ValueError("source instrument is not installed")
        allowed_dimensions = set.intersection(*(INSTRUMENT_DIMENSIONS[name] for name in instruments)) if instruments else set()
        if set(dims) - allowed_dimensions:
            raise ValueError("dimension not emitted by source instrument")
        if len(set(instruments)) != len(instruments):
            raise ValueError("duplicate source instrument")
        if not isinstance(row["source_gap"], str) or len(row["source_gap"]) > 1024:
            raise ValueError("bounded source-gap explanation required")
        kind = row["source_kind"]
        if not isinstance(kind, str) or kind not in {"global_gauge", "process_gauge", "backend_query", "external_required"}:
            raise ValueError("unsupported source kind")
        if not isinstance(row["unit"], str) or row["unit"] not in {"ratio", "seconds", "jobs", "processes", "rows", "boolean", "facts", "objects"}:
            raise ValueError("unsupported SLI unit")
        if kind in {"global_gauge", "process_gauge"}:
            expected_reducer = "max" if kind == "global_gauge" else "min"
            if len(instruments) != 1 or row["replica_reducer"] != expected_reducer or KNOWN_INSTRUMENTS[instruments[0]] != row["unit"]:
                raise ValueError("gauge unit/replica semantics mismatch")
            expected_freshness = "db_snapshot" if kind == "global_gauge" else "transport_observation"
            if row["freshness"] != expected_freshness:
                raise ValueError("source capture policy mismatch")
        elif kind == "external_required":
            if instruments or not row["source_gap"] or row["freshness"] != "external_capture":
                raise ValueError("missing source cannot masquerade as an instrument")
        elif not instruments or not row["source_gap"] or row["freshness"] != "query_capture":
            raise ValueError("verified query binding remains required")
        if not isinstance(row["replica_reducer"], str) or row["replica_reducer"] not in {"max", "min", "sum_rates", "sum_histogram_buckets"}:
            raise ValueError("unsupported replica aggregation")
        if row["capture_policy"] != CAPTURE_POLICIES[row["freshness"]]:
            raise ValueError("capture must use source observation, never export/heartbeat time")
        runbook = row["runbook"]
        if not isinstance(runbook, str) or not re.fullmatch(r"docs/[a-z0-9-]+\.md#[a-z0-9-]+", runbook):
            raise ValueError("local runbook heading required")
        path, anchor = runbook.split("#")
        resolved = (repository / path).resolve(strict=True)
        if not resolved.is_relative_to(repository.resolve()):
            raise ValueError("runbook escapes repository")
        headings = {heading_slug(line.lstrip("# ")) for line in resolved.read_text().splitlines() if line.startswith("#")}
        if anchor not in headings:
            raise ValueError("missing runbook heading: " + runbook)
    return {"valid_catalog": True, "rules": len(rows), "production_activation_permitted": False,
            "unapproved_threshold_rows": len(rows),
            "missing_source_rows": sum(row["source_kind"] == "external_required" for row in rows),
            "verified_ingestion": False, "real_notification_delivery": False}


def panel_instances(row):
    import itertools
    keys = sorted(row["dimensions"])
    values = [sorted(CLOSED_DIMENSIONS[k]) if row["dimensions"][k] == "*" else [row["dimensions"][k]] for k in keys]
    for selected in itertools.product(*values):
        dimensions = dict(zip(keys, selected))
        suffix = "".join(":" + key + "=" + dimensions[key] for key in keys)
        yield row["id"] + suffix, dimensions


def assess_panels(value, snapshot, now):
    if not isinstance(snapshot, dict) or set(snapshot) != {"schema_version", "observations"} or type(snapshot["schema_version"]) is not int or snapshot["schema_version"] != 1:
        raise ValueError("normalized snapshot schema1 required")
    observations = snapshot["observations"]
    if not isinstance(observations, dict) or len(observations) > 200:
        raise ValueError("bounded normalized observations required")
    instances = [(row, panel_id, dims) for row in value["rules"] for panel_id, dims in panel_instances(row)]
    if len(instances) > 200 or set(observations) - {panel_id for _, panel_id, _ in instances}:
        raise ValueError("unknown or excessive observation panel")
    result = []
    for row, panel_id, dims in instances:
        record = {"id": panel_id, "rule": row, "dimensions": dims,
                  "state": "unknown", "value": None, "proposal": None,
                  "fresh_replicas": 0, "rejected_replicas": 0,
                  "expires_at_ms": None, "latest_capture_ms": None}
        if row["source_kind"] in {"external_required", "backend_query"}:
            record["reason"] = row["source_gap"]
        else:
            samples = observations.get(panel_id, [])
            observed = fresh_gauge(samples, now=now, max_age_seconds=row["max_age_seconds"],
                                   reducer=row["replica_reducer"], dimensions=dims)
            record.update(observed)
            record["reason"] = "normalized source capture; ingestion/coverage not verified"
            if observed["state"] == "observed":
                if observed["value"] < 0 or row["unit"] == "boolean" and observed["value"] not in {0, 1}:
                    raise ValueError("impossible normalized gauge value")
                above = row["comparison"] == "above"
                compare = (lambda threshold: observed["value"] > threshold) if above else (lambda threshold: observed["value"] < threshold)
                record["proposal"] = "page" if compare(row["page"]) else "ticket" if compare(row["ticket"]) else None
                captures = [timestamp(s["observed_at"]) for s in samples if s["complete"] and 0 <= (now - timestamp(s["observed_at"])).total_seconds() <= row["max_age_seconds"]]
                record["expires_at_ms"] = (min(captures).timestamp() + row["max_age_seconds"]) * 1000
                record["latest_capture_ms"] = max(captures).timestamp() * 1000
        result.append(record)
    return result


def render_html(value, snapshot, repository, now, runbook_prefix="../../"):
    import html
    validate_catalog(value, repository)
    panels = assess_panels(value, snapshot, now)
    rows = []
    for p in panels:
        rule = p["rule"]
        attrs = ""
        if p["expires_at_ms"] is not None:
            attrs = f' data-expire-at="{p["expires_at_ms"]}" data-captured-at="{p["latest_capture_ms"]}"'
        reading = "unknown" if p["value"] is None else str(p["value"])
        proposal = "" if p["proposal"] is None else "Unapproved " + p["proposal"] + " proposal"
        cells = [html.escape(p["id"]), html.escape(rule["title"]),
                 '<span class="reading">' + html.escape(reading) + '</span> ' + html.escape(rule["unit"]),
                 '<span class="status">' + p["state"] + '</span>',
                 '<span class="proposal">' + html.escape(proposal) + '</span>',
                 html.escape(str(p["fresh_replicas"]) + " fresh / " + str(p["rejected_replicas"]) + " rejected"),
                 html.escape(rule["owner_role"]), '<a href="' + html.escape(runbook_prefix + rule["runbook"], quote=True) + '">' + html.escape(rule["runbook"]) + '</a>',
                 html.escape(p["reason"])]
        rows.append("<tr" + attrs + "><td>" + "</td><td>".join(cells) + "</td></tr>")
    rendered = html.escape(now.isoformat())
    return '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Local operations evidence dashboard</title>
<style>body{font:16px system-ui;margin:2rem;color:CanvasText;background:Canvas}table{border-collapse:collapse;width:100%}th,td{text-align:left;vertical-align:top;border-bottom:1px solid;padding:.6rem;font-size:.85rem}caption{text-align:left;margin:1rem 0}code{overflow-wrap:anywhere}tr[data-expired]{opacity:.7}</style>
<h1>Local operations evidence</h1>
<p><strong>Unapproved proposals. Production activation is disabled.</strong> No backend ingestion, delivered production alert, fleet drain or business authority is established by this artifact.</p>
<p>Rendered as of ''' + rendered + '''. Source capture timestamps are retained; expiry only changes observations to unknown. There is no background network request. Re-render with a fresh normalized snapshot to restore observations. Proposed actions compare this capture only; a sustained rule window is not established.</p>
<label for="panel-filter">Filter panels</label>
<input id="panel-filter" type="search" maxlength="80" aria-controls="panel-table">
<p id="panel-count" role="status" aria-live="polite" aria-atomic="true">All panels shown.</p>
<table id="panel-table"><caption>Closed dimensions only. Global database gauges use max across complete fresh replicas; process transport readiness uses min. Counter/histogram and missing-source rows remain unknown until verified adapters exist.</caption>
<thead><tr><th>Panel</th><th>Meaning</th><th>Value</th><th>State</th><th>Proposed action</th><th>Coverage</th><th>Owner role</th><th>Runbook</th><th>Evidence limit</th></tr></thead><tbody>''' + "\n".join(rows) + '''</tbody></table>
<script>
const panelFilter=document.getElementById('panel-filter');
const panelCount=document.getElementById('panel-count');
function filterPanels(){
  panelFilter.value=panelFilter.value.slice(0,80);
  const query=panelFilter.value.trim().toLowerCase();
  const rows=document.querySelectorAll('tbody tr');
  let shown=0;
  for(const row of rows){
    row.hidden=!row.textContent.toLowerCase().includes(query);
    if(!row.hidden)shown++;
  }
  const count=`Showing ${shown} of ${rows.length} panels`;
  if(panelCount.textContent!==count)panelCount.textContent=count;
}
panelFilter.addEventListener('input',filterPanels);
function expireStaleObservations(){const now=Date.now();for(const row of document.querySelectorAll('tr[data-expire-at]')){const expiry=Number(row.dataset.expireAt),capture=Number(row.dataset.capturedAt);if(!Number.isFinite(expiry)||!Number.isFinite(capture)||now>expiry||now<capture){row.dataset.expired='true';row.querySelector('.reading').textContent='unknown';row.querySelector('.status').textContent='unknown';row.querySelector('.proposal').textContent='';}}filterPanels();}
expireStaleObservations();setInterval(expireStaleObservations,1000);
</script></html>'''


def read_json_bounded(path):
    import json
    import os
    import stat
    def unique_fields(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON field")
            result[key] = value
        return result

    def reject_constant(_):
        raise ValueError("nonfinite JSON value")

    # Nonblocking open permits the regular-file check without hanging on a FIFO.
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "rb") as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            raise ValueError("regular JSON file required")
        data = source.read(262145)
    if len(data) > 262144:
        raise ValueError("JSON artifact exceeds 256KiB limit")
    return json.loads(data.decode("utf-8"), object_pairs_hook=unique_fields,
                      parse_constant=reject_constant)


def main():
    import argparse
    import json
    import os
    from pathlib import Path
    parser = argparse.ArgumentParser(description="Render local unapproved operations evidence, without network access or backend activation.")
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--production", action="store_true")
    args = parser.parse_args()
    if args.production:
        parser.exit(2, "production activation requires selected/verified ingestion, signed targets and real delivered routes; this tool emits local proposals only\n")
    try:
        value = read_json_bounded(args.catalog)
        summary = validate_catalog(value, args.repository)
        snapshot = read_json_bounded(args.snapshot) if args.snapshot else {"schema_version": 1, "observations": {}}
        now = datetime.now(timezone.utc)
        panels = assess_panels(value, snapshot, now)
        prefix = os.path.relpath(args.repository.resolve(), args.output.resolve()).rstrip("/") + "/"
        html = render_html(value, snapshot, args.repository, now, prefix)
        # A new output directory prevents accidental report/source overwrite.
        args.output.mkdir(mode=0o700)
        with (args.output / "dashboard.html").open("x", encoding="utf-8") as output:
            output.write(html)
        with (args.output / "rules.proposed.json").open("x", encoding="utf-8") as output:
            json.dump(value, output, indent=2)
            output.write("\n")
        summary.update({"rendered_at": now.isoformat(), "panels": len(panels),
                        "unknown_panels": sum(p["state"] == "unknown" for p in panels),
                        "production_proof": False, "network_requests": 0,
                        "source_capture_adapter_verified": False,
                        "sustained_window_verified": False,
                        "panel_assessments": [{key: panel[key] for key in (
                            "id", "state", "value", "proposal", "dimensions", "reason",
                            "fresh_replicas", "rejected_replicas", "expires_at_ms", "latest_capture_ms"
                        )} for panel in panels]})
        with (args.output / "assessment.json").open("x", encoding="utf-8") as output:
            json.dump(summary, output, indent=2)
            output.write("\n")
    except (ValueError, OSError, TypeError, RecursionError):
        parser.exit(2, "invalid/incomplete local catalog, snapshot or output; no production activation\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
