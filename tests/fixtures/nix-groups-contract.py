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
HEAD = 'a' * 40
MEMBERS = ['.#checks.x86_64-linux.client', '.#checks.x86_64-linux.recovery']


class Groups(unittest.TestCase):
    def execute(self, *, fail=False, malformed=False, missing=False, foreign=False, pr=False, fresh=False, warm=False, fresh_failure=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            builds = []
            valid_outputs = {MEMBERS[0]} if warm else set()
            fresh_builders = []

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
                self.assertEqual(command[0:2], ['nix', 'build'])
                self.assertIn('--print-build-logs', command)
                self.assertEqual(command[-5:-1], ['--max-jobs', '1', '--cores', '2'])
                builds.append(command[-1])
                if '--rebuild' in command:
                    if command[-1] not in valid_outputs:
                        return subprocess.CompletedProcess(command, 9, b'')
                    fresh_builders.append(command[-1])
                    if fresh_failure:
                        return subprocess.CompletedProcess(command, 7, b'')
                else:
                    valid_outputs.add(command[-1])
                data = [{'drvPath': '/nix/store/result.drv', 'outputs': {'out': '/nix/store/result'}}]
                kwargs['stderr'].write(b'retained builder diagnostic\n')
                return subprocess.CompletedProcess(command, 7 if fail and len(builds) == 1 else 0,
                                                   b'not-json' if malformed else json.dumps(data).encode())

            env = {'SIMIT_NIX_BUILD_RESULTS': str(root), 'INSTALLABLES': json.dumps(MEMBERS),
                   'SIMIT_NIX_PLAN': json.dumps(MEMBERS), 'SIMIT_NIX_CHECK_SYSTEMS': '["x86_64-linux"]',
                   'SIMIT_NIX_LIMITS': '["--max-jobs","1","--cores","2"]',
                   'GITHUB_SHA': 'c' * 40 if foreign else HEAD, 'GITHUB_EVENT_NAME': 'push',
                   'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1'}
            if fresh:
                env['SIMIT_NIX_FRESH'] = json.dumps([MEMBERS[0]])
            if pr:
                event = root / 'event.json'
                event.write_text(json.dumps({'pull_request': {'head': {'sha': HEAD}}}))
                env.update(GITHUB_EVENT_NAME='pull_request', GITHUB_SHA='c' * 40,
                           GITHUB_EVENT_PATH=str(event))
            with patch.dict(os.environ, env), patch('subprocess.check_output', output), patch('subprocess.run', run):
                try:
                    exec(compile(PROGRAM, '<generated-group>', 'exec'), {})
                except SystemExit as error:
                    status = error.code
                except AssertionError:
                    status = 1
            receipt = json.loads((root / 'receipt.json').read_text()) if (root / 'receipt.json').exists() else None
            if receipt is not None:
                receipt['fixture_fresh_builders'] = fresh_builders
            logs = [p.read_text() for p in root.glob('*/build.log')]
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

    def test_pr_head_is_qualified_separately_from_synthetic_merge(self):
        status, builds, receipt, _ = self.execute(pr=True)
        self.assertEqual(status, 0)
        self.assertEqual(builds, MEMBERS)
        self.assertEqual(receipt['head'], HEAD)
        self.assertEqual(receipt['event'], 'pull_request')

    def test_cold_and_warm_fresh_outputs_both_execute_current_worker_builder(self):
        for warm in [False, True]:
            with self.subTest(warm=warm):
                status, builds, receipt, _ = self.execute(fresh=True, warm=warm)
                self.assertEqual(status, 0)
                self.assertEqual(builds, [MEMBERS[0], MEMBERS[0], MEMBERS[1]])
                self.assertEqual(receipt['fixture_fresh_builders'], [MEMBERS[0]])
                self.assertTrue(receipt['results'][0]['fresh_execution'])

    def test_fresh_builder_failure_remains_a_failure_after_successful_materialization(self):
        status, _, receipt, _ = self.execute(fresh=True, fresh_failure=True)
        self.assertEqual(status, 1)
        self.assertFalse(receipt['success'])
        self.assertEqual(receipt['results'][0]['exit_status'], 7)

    def test_malformed_success_is_not_accepted_and_other_outputs_still_run(self):
        status, builds, receipt, _ = self.execute(malformed=True)
        self.assertEqual(status, 1)
        self.assertEqual(builds, MEMBERS)
        self.assertFalse(receipt['success'])


unittest.main()
