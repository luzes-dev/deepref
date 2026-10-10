#!/usr/bin/env python3
"""Reproduce terminal V1 inference and sampling from a Deepref audit CSV.

No model calls or third-party dependencies. This checks a declared human
reference, not true recall or whether self-asserted actors are distinct people.
"""
import argparse
import csv
import hashlib
import json
import math
import sys
import uuid
from fractions import Fraction
from pathlib import Path


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, name, low, high):
    require(type(value) is int and low <= value <= high, f"invalid {name}")
    return value


def inference(population, retained, sample, observed, target, alpha):
    cutoff = ((retained + observed) * 100) // target - retained + 1
    if cutoff > population:
        probability = Fraction(0)
    else:
        denominator = math.comb(population, sample)
        numerator = sum(
            math.comb(cutoff, k) * math.comb(population - cutoff, sample - k)
            for k in range(max(0, sample - (population - cutoff)), min(observed, cutoff, sample) + 1)
        )
        probability = Fraction(numerator, denominator)
    passed = sample == population or probability <= Fraction(alpha, 1_000_000_000)
    return cutoff, probability, passed


def sample_ids(ids, seed, domain="ai-first-fisher-yates-v1"):
    ids = sorted(ids, key=lambda value: uuid.UUID(value).bytes)
    counter = 0
    for i in range(len(ids) - 1, 0, -1):
        size = i + 1
        upper = (2**64 - 1) - (2**64 - 1) % size
        while True:
            data = f"{domain}:{uuid.UUID(seed)}:{counter}".encode()
            value = int.from_bytes(hashlib.sha256(data).digest()[:8], "big")
            counter += 1
            if value < upper:
                break
        j = value % size
        ids[i], ids[j] = ids[j], ids[i]
    return ids


def reproduce(cohort, labels):
    require(cohort["sampling_algorithm"] in ("sha256-counter-rejection-fisher-yates-v1", "sha256-counter-rejection-fisher-yates-controls-v2"), "unsupported sample algorithm")
    require(cohort["policy_version"] == 1, "unsupported eligibility policy")
    ordinal = integer(cohort["audit_ordinal"], "audit ordinal", 1, 25)
    alpha = integer(cohort["alpha_billionths"], "alpha", 1, 50_000_000)
    require(alpha == 50_000_000 >> ordinal, "alpha does not match project ordinal")
    population = integer(cohort["population"], "population", 1, 1_000_000)
    retained = integer(cohort["reference_relevant"], "reference count", 0, 1_000_000)
    sample = integer(cohort["sample_size"], "sample size", 0, population)
    target = cohort["target_percent"]
    require(target in (95, 98), "unsupported target")
    frame = cohort["frame_snapshot"]
    require(isinstance(frame, list), "missing frozen frame")
    all_ids = [member["report_id"] for member in frame]
    require(len(set(all_ids)) == len(all_ids), "duplicate frozen members")
    x_ids = [member["report_id"] for member in frame if member["quarantine_frame"]]
    sampled = {member["report_id"] for member in frame if member["sampled"]}
    require(len(x_ids) == population and len(sampled) == sample, "frame count mismatch")
    require(sampled == set(sample_ids(x_ids, cohort["sampling_seed"])[:sample]), "seed does not reproduce sample")
    reference = cohort["reference_snapshot"]
    reference_ids = [row["report_id"] for row in reference]
    require(len(set(reference_ids)) == len(reference_ids), "duplicate human reference")
    require(set(reference_ids) == set(all_ids) - set(x_ids), "human reference frame mismatch")
    require(all(row["decision"] in ("include", "maybe", "exclude") for row in reference), "incomplete human reference")
    require(retained == sum(row["decision"] in ("include", "maybe") for row in reference), "frozen R mismatch")
    controls = {member["report_id"] for member in frame if member.get("audit_control", False)}
    if cohort["sampling_algorithm"].endswith("controls-v2"):
        require(controls == set(sample_ids(reference_ids, cohort["sampling_seed"], "ai-first-controls-v1")[:sample]), "control seed mismatch")
        require(not controls.intersection(x_ids), "control in quarantine")
        order = {member["report_id"]: member["audit_order"] for member in frame if member["sampled"] or member.get("audit_control", False)}
        expected_order = sample_ids(list(sampled | controls), cohort["sampling_seed"], "ai-first-blind-queue-v1")
        require(all(order[report] == position for position, report in enumerate(expected_order)), "blind queue seed mismatch")
    grouped = {report: [] for report in sampled | controls}
    for label in labels:
        require(label["report_id"] in grouped, "label outside frozen audit tasks")
        grouped[label["report_id"]].append(label)
    for pair in grouped.values():
        require(len(pair) == 2 and len({row["actor_id"] for row in pair}) == 2, "two distinct reference identities required")
        require(all(row["actor_id"].strip() and row["decision"] in ("include", "maybe", "exclude") for row in pair), "invalid reference label")
    observed = sum(any(row["decision"] in ("include", "maybe") for row in pair) for report, pair in grouped.items() if report in sampled)
    cutoff, probability, passed = inference(population, retained, sample, observed, target, alpha)
    stored = cohort["result"]
    require(stored["observed_relevant"] == observed and stored["first_unsafe_total"] == cutoff and stored["passed"] == passed, "stored result does not reproduce")
    require(math.isfinite(stored["p_value"]) and abs(stored["p_value"] - float(probability)) <= 1e-8, "diagnostic CDF mismatch")
    expected_point = None if retained + observed == 0 or sample == 0 else Fraction((retained + observed) * sample, retained * sample + observed * population)
    require((stored["reference_retention"] is None and expected_point is None) or
            (expected_point is not None and stored["reference_retention"] is not None and
             abs(stored["reference_retention"] - float(expected_point)) <= 1e-12), "point estimate mismatch")
    return {
        "cohort_id": cohort["id"], "status": cohort["status"],
        "claim": "conditional_reference_retention_only", "actor_identity": "self_asserted",
        "population": population, "retained_reference": retained, "sample_size": sample, "control_count": len(controls),
        "observed_relevant": observed, "audit_ordinal": ordinal, "alpha_billionths": alpha,
        "p_numerator": str(probability.numerator), "p_denominator": str(probability.denominator),
        "passed": passed, "currently_finalized": cohort["status"] == "finalized",
        "semantic_bundle_hash": cohort["semantic_bundle_hash"],
    }


def self_test():
    require(inference(40, 0, 39, 0, 95, 25_000_000)[2], "exact tie must pass")
    require(not inference(40, 0, 39, 0, 95, 24_999_999)[2], "one billionth below tie must fail")
    require(inference(40, 0, 40, 40, 98, 1)[2], "census must pass")
    require(inference(40, 10_000, 0, 0, 95, 1)[2], "empty null must pass")
    ids = [str(uuid.UUID(int=i)) for i in range(1, 101)]
    seed = str(uuid.UUID(int=123))
    shuffled = sample_ids(ids, seed)
    require(shuffled != ids and sorted(shuffled) == ids, "sample must be a permutation")
    require(shuffled == sample_ids(list(reversed(ids)), seed), "canonical order must reproduce")
    print("6 reproducibility self-checks passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audit_csv", nargs="?", type=Path)
    parser.add_argument("--cohort", help="cohort UUID; defaults to all evaluated terminal cohorts")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    require(args.audit_csv is not None, "supply an audit CSV or --self-test")
    with args.audit_csv.open(newline="", encoding="utf-8-sig") as stream:
        rows = list(csv.DictReader(stream))
    cohorts = [json.loads(row["payload"]) for row in rows if row["event_type"] == "ai_first_cohort"]
    cohorts = [cohort for cohort in cohorts if cohort.get("result") is not None and (args.cohort is None or cohort["id"] == args.cohort)]
    require(cohorts, "no evaluated terminal cohort found; active blinded evidence is not exported")
    labels = [json.loads(row["payload"]) for row in rows if row["event_type"] == "ai_first_reference_label"]
    results = [reproduce(cohort, [label for label in labels if label["cohort_id"] == cohort["id"]]) for cohort in cohorts]
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(f"audit reproduction refused: {error}", file=sys.stderr)
        sys.exit(1)
