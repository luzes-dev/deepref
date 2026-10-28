#!/usr/bin/env python3
"""Keep raw OSV findings; classify one verified backport without global ignores."""
from datetime import date
import json
from pathlib import Path
import subprocess
import sys

ADVISORY = 'GHSA-gcqx-w48m-w2h4'


def classify(report: dict, root: Path) -> tuple[list[dict], list[dict]]:
    remaining, patched = [], []
    results = report['results']
    if not isinstance(results, list):
        raise ValueError('unknown scanner results shape')
    for result in results:
        source = result['source']
        path = Path(source['path'])
        path = path if path.is_absolute() else root / path
        for item in result['packages']:
            package = item['package']
            vulnerabilities = item['vulnerabilities']
            if not isinstance(vulnerabilities, list):
                raise ValueError('unknown vulnerability results shape')
            for vulnerability in vulnerabilities:
                identifier = vulnerability['id']
                if not isinstance(identifier, str) or not identifier:
                    raise ValueError('missing vulnerability identity')
                finding = {'source': source['path'], 'package': package, 'id': identifier}
                exact = (source['type'] == 'lockfile'
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
        ['osv-scanner', 'scan', 'source', '--recursive', '--config', 'osv-scanner.toml', '--format=json', '.'],
        cwd=root, text=True, capture_output=True,
    )
    # Preserve unmodified machine-readable results in the probe log, including
    # the backported version's advisory. Never turn scanner errors into success.
    print(result.stdout, end='', flush=True)
    print(result.stderr, file=sys.stderr, end='', flush=True)
    if result.returncode not in (0, 1):
        return result.returncode
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
