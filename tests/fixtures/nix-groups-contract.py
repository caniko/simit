"""Exercise the generated group program at its Git/Nix process boundaries."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

PROGRAM = Path(sys.argv.pop()).read_text()
FALLBACK = Path(sys.argv.pop()).read_text()
REAL_RUN = subprocess.run
HEAD = 'a' * 40
MEMBERS = ['.#checks.x86_64-linux.client', '.#checks.x86_64-linux.recovery']


class Groups(unittest.TestCase):
    def execute(self, *, fail=False, malformed=False, missing=False, foreign=False, pr=False, optimized=False, hook_fail=False, setup_fail=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            builds = []

            def output(command, **kwargs):
                if command[0] == 'git':
                    return HEAD if command[-1] == 'HEAD' else 'b' * 40
                self.assertEqual(command[:4], ['nix', 'eval', '--json', '--no-update-lock-file'])
                return json.dumps({'client': '/nix/store/client.drv',
                                   'recovery': '/nix/store/recovery.drv',
                                   **({'new-check': '/nix/store/new.drv'} if missing else {})})

            def run(command, **kwargs):
                if command[0] == 'git':
                    return subprocess.CompletedProcess(command, 0)
                if command[0] == 'bash':
                    if hook_fail:
                        return subprocess.CompletedProcess(command, 19)
                    return REAL_RUN(command, **kwargs)
                self.assertEqual(command[0:2], ['nix', 'build'])
                self.assertIn('--print-build-logs', command)
                self.assertEqual(command[-5:-1], ['--max-jobs', '1', '--cores', '2'])
                builds.append(command[-1])
                data = [{'drvPath': '/nix/store/result.drv', 'outputs': {'out': '/nix/store/result'}}]
                kwargs['stderr'].write(b'retained builder diagnostic\n')
                return subprocess.CompletedProcess(command, 7 if fail and len(builds) == 1 else 0,
                                                   b'not-json' if malformed else json.dumps(data).encode())

            env = {'SIMIT_NIX_BUILD_RESULTS': str(root), 'INSTALLABLES': json.dumps(MEMBERS),
                   'SIMIT_NIX_PLAN': json.dumps(MEMBERS), 'SIMIT_NIX_CHECK_SYSTEMS': '["x86_64-linux"]',
                   'SIMIT_NIX_LIMITS': '["--max-jobs","1","--cores","2"]',
                   'GITHUB_SHA': 'c' * 40 if foreign else HEAD, 'GITHUB_EVENT_NAME': 'push',
                    'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1'}
            env['SIMIT_NIX_GROUP'] = 'network'
            env.update(SIMIT_NIX_POST_BUILD=json.dumps(["test -s \"$SIMIT_NIX_BUILD_RESULTS/result.json\" && test -s \"$SIMIT_NIX_BUILD_RESULTS/build.log\" && printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-member\""]),
                       SIMIT_NIX_POST_BUILD_ALWAYS=json.dumps(["printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-diagnostic\""]))
            if pr:
                event = root / 'event.json'
                event.write_text(json.dumps({'pull_request': {'head': {'sha': HEAD}}}))
                env.update(GITHUB_EVENT_NAME='pull_request', GITHUB_SHA='c' * 40,
                           GITHUB_EVENT_PATH=str(event))
            with patch.dict(os.environ, env), patch('subprocess.check_output', output), patch('subprocess.run', run):
                try:
                    if setup_fail:
                        raise SystemExit(1)
                    exec(compile(PROGRAM, '<generated-group>', 'exec', optimize=2 if optimized else 0), {})
                except SystemExit as error:
                    status = error.code
                except (AssertionError, RuntimeError):
                    status = 1
                collected = REAL_RUN(['bash', '-e', '-o', 'pipefail', '-c', FALLBACK])
                self.assertEqual(collected.returncode, 0)
            receipt = json.loads((root / 'receipt.json').read_text()) if (root / 'receipt.json').exists() else None
            logs = [p.read_text() for p in root.glob('*/build.log')]
            if not hook_fail:
                for index, member in enumerate(MEMBERS):
                    self.assertEqual((root / str(index) / 'hook-diagnostic').read_text(), member)
            if receipt and receipt.get('complete') and not hook_fail:
                for index, row in enumerate(receipt['results']):
                    folder = root / str(index)
                    self.assertEqual((folder / 'hook-diagnostic').read_text(), MEMBERS[index])
                    self.assertFalse((folder / 'fallback-diagnostic-0.status').exists())
                    self.assertEqual((folder / 'hook-member').exists(), row['exit_status'] == 0 and row['output_map_valid'])
                    if (folder / 'hook-member').exists():
                        self.assertEqual((folder / 'hook-member').read_text(), MEMBERS[index])
            return status, builds, receipt, logs

    def test_failure_retains_all_outputs_and_diagnostics(self):
        status, builds, receipt, logs = self.execute(fail=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, MEMBERS)
        self.assertTrue(receipt['complete'])
        self.assertFalse(receipt['success'])
        self.assertEqual([r['exit_status'] for r in receipt['results']], [7, 0])
        self.assertEqual(logs, ['retained builder diagnostic\n'] * 2)

    def test_success_has_one_receipt_per_enrolled_output(self):
        status, builds, receipt, _ = self.execute()
        self.assertEqual(status, 0)
        self.assertEqual(builds, MEMBERS)
        self.assertTrue(receipt['success'])
        self.assertEqual(receipt['head'], HEAD)

    def test_new_unassigned_check_rejects_before_realization(self):
        status, builds, receipt, _ = self.execute(missing=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, [])
        self.assertFalse(receipt['complete'])

    def test_wrong_source_rejects_before_realization(self):
        status, builds, receipt, _ = self.execute(foreign=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, [])
        self.assertIsNone(receipt)

    def test_optimized_python_cannot_disable_source_or_coverage_admission(self):
        for kwargs in [{'foreign': True}, {'missing': True}]:
            status, builds, _, _ = self.execute(optimized=True, **kwargs)
            self.assertEqual(status, 1)
            self.assertEqual(builds, [])

    def test_post_build_failure_fails_group_but_other_outputs_still_run(self):
        status, builds, receipt, _ = self.execute(hook_fail=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, MEMBERS)
        self.assertFalse(receipt['success'])

    def test_failed_setup_collects_member_diagnostics_without_python_or_realization(self):
        status, builds, receipt, _ = self.execute(setup_fail=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, [])
        self.assertIsNone(receipt)

    def test_pr_head_is_qualified_separately_from_synthetic_merge(self):
        status, builds, receipt, _ = self.execute(pr=True)
        self.assertEqual(status, 0)
        self.assertEqual(builds, MEMBERS)
        self.assertEqual(receipt['head'], HEAD)
        self.assertEqual(receipt['event'], 'pull_request')

    def test_malformed_success_is_not_accepted_and_other_outputs_still_run(self):
        status, builds, receipt, _ = self.execute(malformed=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, MEMBERS)
        self.assertFalse(receipt['success'])


unittest.main()
