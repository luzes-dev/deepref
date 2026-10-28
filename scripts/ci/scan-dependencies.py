#!/usr/bin/env python3
"""Keep raw OSV findings; classify one verified backport without global ignores."""
from datetime import date
import json
from pathlib import Path
import subprocess
import sys

ADVISORY = 'GHSA-gcqx-w48m-w2h4'


def expect_object(value, label):
    if not isinstance(value, dict):
        raise ValueError('unknown ' + label + ' object shape')
    return value


def expect_array(value, label):
    if not isinstance(value, list):
        raise ValueError('unknown ' + label + ' array shape')
    return value


def expect_string(value, label):
    if not isinstance(value, str) or not value.strip():
        raise ValueError('unknown ' + label + ' string shape')
    return value


def classify(report: dict, root: Path) -> tuple[list[dict], list[dict]]:
    remaining, patched = [], []
    report = expect_object(report, 'report')
    for result in expect_array(report['results'], 'results'):
        result = expect_object(result, 'result')
        source = expect_object(result['source'], 'source')
        source_type = expect_string(source['type'], 'source type')
        source_path = expect_string(source['path'], 'source path')
        path = Path(source_path)
        path = path if path.is_absolute() else root / path
        for item in expect_array(result['packages'], 'packages'):
            item = expect_object(item, 'package result')
            package = expect_object(item['package'], 'package')
            for field in ('name', 'version', 'ecosystem'):
                expect_string(package[field], 'package ' + field)
            if 'purl' in package:
                expect_string(package['purl'], 'package purl')
            for vulnerability in expect_array(item['vulnerabilities'], 'vulnerabilities'):
                vulnerability = expect_object(vulnerability, 'vulnerability')
                identifier = expect_string(vulnerability['id'], 'vulnerability id')
                finding = {'source': source_path, 'package': package, 'id': identifier}
                exact = (source_type == 'lockfile'
                         and path.resolve() == (root / 'pnpm-lock.yaml').resolve()
                         and package['ecosystem'] == 'npm'
                         and package['name'] == 'deepmerge'
                         and package['version'] == '4.3.1'
                         and package.get('purl') == 'pkg:npm/deepmerge@4.3.1'
                         and identifier == ADVISORY)
                (patched if exact else remaining).append(finding)
    return remaining, patched


def scan(root: Path, run=subprocess.run, today=None) -> int:
    identity = run(
        ['node', str(root / 'scripts/ci/check-deepmerge-patch.mjs')],
        cwd=root, text=True, capture_output=True, timeout=45,
    )
    if identity.returncode:
        print(identity.stderr, file=sys.stderr, end='')
        return identity.returncode
    evidence = json.loads(identity.stdout)
    if evidence['advisory'] != ADVISORY or evidence['package'] != 'deepmerge' or evidence['version'] != '4.3.1':
        raise ValueError('unknown patched-source disposition')
    if (today or date.today()) >= date.fromisoformat(evidence['expires']):
        raise ValueError('patched-source disposition expired; review or upgrade required')
    contracts = run(
        ['node', '--test', str(root / 'scripts/ci/test-deepmerge-contract.mjs')],
        cwd=root, timeout=45,
    )
    if contracts.returncode:
        return contracts.returncode
    result = run(
        ['osv-scanner', 'scan', 'source', '--recursive', '--config', 'osv-scanner.toml', '--verbosity=error', '--format=json', '.'],
        cwd=root, text=True, capture_output=True,
    )
    # Preserve unmodified machine-readable results in the probe log, including
    # the backported version's advisory. Never turn scanner errors into success.
    print(result.stdout, end='', flush=True)
    print(result.stderr, file=sys.stderr, end='', flush=True)
    if result.returncode not in (0, 1):
        return result.returncode
    # OSV 2.7 can return vulnerability exit 1 even after an extraction error.
    # Error-only verbosity makes every stderr diagnostic a refusal, without
    # relying on fragile message prefixes or suppressing incomplete coverage.
    if result.stderr:
        raise ValueError('scanner emitted error-level diagnostics; scan is incomplete')
    remaining, patched = classify(json.loads(result.stdout), root)
    if result.returncode == 1 and not remaining and not patched:
        raise ValueError('scanner failed without an explained finding')
    print('\nDependency disposition: ' + json.dumps({
        'verified_patch': evidence, 'patched_findings': patched,
        'unresolved_findings': remaining,
    }), flush=True)
    return 1 if remaining else 0


if __name__ == '__main__':
    try:
        sys.exit(scan(Path(__file__).resolve().parents[2]))
    except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        print('Dependency scan refused: ' + str(error), file=sys.stderr)
        sys.exit(2)
