"""Security gate contracts: only one exact, verified backport can be classified."""
from contextlib import redirect_stdout, redirect_stderr
from copy import deepcopy
from datetime import date
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('dependency_scan', ROOT / 'scripts/ci/scan-dependencies.py')
scan = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scan)
EVIDENCE = {'advisory': scan.ADVISORY, 'package': 'deepmerge', 'version': '4.3.1',
            'patch_sha256': '658bcc27776e130ae51c0ad9597d4a3a52ac867af0fc014b78714ff162845fe0',
            'expires': '2027-01-26'}


def report():
    return {'results': [{'source': {'path': str(ROOT / 'pnpm-lock.yaml'), 'type': 'lockfile'},
                         'packages': [{'package': {'name': 'deepmerge', 'version': '4.3.1', 'ecosystem': 'npm', 'purl': 'pkg:npm/deepmerge@4.3.1'},
                                       'vulnerabilities': [{'id': scan.ADVISORY}]}]}]}


class DependencyPatchTests(unittest.TestCase):
    def invoke(self, raw, identity=0, contracts=0, scanner=1, today=date(2026, 10, 28)):
        calls = []

        def run(command, **options):
            calls.append(command)
            if len(calls) == 1:
                return subprocess.CompletedProcess(command, identity, json.dumps(EVIDENCE), 'identity refused\n')
            if len(calls) == 2:
                return subprocess.CompletedProcess(command, contracts)
            self.assertEqual(command, ['osv-scanner', 'scan', 'source', '--recursive', '--config',
                                       'osv-scanner.toml', '--format=json', '.'])
            return subprocess.CompletedProcess(command, scanner, json.dumps(raw), '')

        output = io.StringIO()
        with redirect_stdout(output), redirect_stderr(io.StringIO()):
            status = scan.scan(ROOT, run=run, today=today)
        return status, calls, output.getvalue()

    def test_only_exact_source_package_version_and_advisory_can_be_classified(self):
        self.assertEqual(len(scan.classify(report(), ROOT)[1]), 1)
        for key, value in [('name', 'another-package'), ('version', '4.2.0'), ('ecosystem', 'another-ecosystem'), ('purl', 'pkg:npm/another-package@4.3.1')]:
            with self.subTest(key=key):
                raw = report()
                raw['results'][0]['packages'][0]['package'][key] = value
                self.assertEqual(len(scan.classify(raw, ROOT)[0]), 1)
        for key, value in [('path', str(ROOT / 'other/pnpm-lock.yaml')), ('type', 'sbom')]:
            raw = report()
            raw['results'][0]['source'][key] = value
            self.assertEqual(len(scan.classify(raw, ROOT)[0]), 1)
        raw = report()
        raw['results'][0]['packages'][0]['vulnerabilities'][0]['id'] = 'GHSA-other'
        self.assertEqual(len(scan.classify(raw, ROOT)[0]), 1)

    def test_new_advisory_on_same_package_still_blocks_and_raw_evidence_remains(self):
        raw = report()
        raw['results'][0]['packages'][0]['vulnerabilities'].append({'id': 'GHSA-other'})
        status, calls, output = self.invoke(raw)
        self.assertEqual(status, 1)
        self.assertEqual(len(calls), 3)
        self.assertIn(scan.ADVISORY, output)
        self.assertIn('GHSA-other', output)

    def test_other_package_findings_remain_blocking(self):
        raw = report()
        other = deepcopy(raw['results'][0]['packages'][0])
        other['package'] = {'name': 'unresolved-package', 'version': '1.0.0', 'ecosystem': 'npm'}
        raw['results'][0]['packages'].append(other)
        self.assertEqual(self.invoke(raw)[0], 1)

    def test_verified_backport_only_can_pass(self):
        status, calls, output = self.invoke(report())
        self.assertEqual(status, 0)
        self.assertEqual(len(calls), 3)
        self.assertIn('patched_findings', output)
        self.assertIn('unresolved_findings', output)

    def test_unpatched_identity_or_behavior_never_calls_scanner(self):
        for identity, contracts, expected in [(7, 0, 1), (0, 8, 2)]:
            status, calls, _ = self.invoke(report(), identity=identity, contracts=contracts)
            self.assertNotEqual(status, 0)
            self.assertEqual(len(calls), expected)

    def test_expired_disposition_fails_closed(self):
        with self.assertRaisesRegex(ValueError, 'expired'):
            self.invoke(report(), today=date(2027, 1, 26))

    def test_scanner_errors_and_unexplained_failures_are_not_suppressed(self):
        self.assertEqual(self.invoke(report(), scanner=128)[0], 128)
        with self.assertRaisesRegex(ValueError, 'without an explained'):
            self.invoke({'results': []})
        with self.assertRaises(KeyError):
            self.invoke({'unknown': []})

    def test_manifest_runs_guarded_scan_and_original_config_has_no_new_ignore(self):
        manifest = tomllib.loads((ROOT / 'scripts/ci/diagnostics.toml').read_text())
        probe = next(item for item in manifest['check'] if item['id'] == 'security.osv')
        self.assertEqual(probe['command'], ['python3', 'scripts/ci/scan-dependencies.py'])
        self.assertNotIn(scan.ADVISORY, (ROOT / 'osv-scanner.toml').read_text())


if __name__ == '__main__':
    unittest.main()
