"""The transport's import boundary rejects provenance/bytes before store effects."""
import gzip
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

PROGRAM = Path(sys.argv.pop()).read_text()
HEAD, TREE = 'a' * 40, 'b' * 40
OUT = '/nix/store/' + '0' * 32 + '-input'
DRV = '/nix/store/' + '1' * 32 + '-input.drv'
DEFINITION = {'drvPath': DRV, 'outputs': {'out': OUT}}


class Transport(unittest.TestCase):
    def execute(self, change=None, alias=False, transitive=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / 'closure.nar.gz'
            data = b'fixture nar bytes'
            archive.write_bytes(gzip.compress(data))
            manifest = {'schema': 'simit.nix-dependencies.v1', 'head': HEAD, 'tree': TREE,
                        'repository': 'owner/repo', 'run': '123', 'attempt': 1,
                        'flake_lock_sha256': None, 'complete': True,
                        'definitions': {'.#input': DEFINITION}, 'closure': [OUT],
                        'uncompressed_bytes': len(data), 'archive_bytes': archive.stat().st_size,
                        'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest()}
            if change:
                manifest[change[0]] = change[1]
            (root / 'manifest.json').write_text(json.dumps(manifest))
            effects = []

            def output(command, **kwargs):
                if command[0] == 'git':
                    return HEAD if command[-1] == 'HEAD' else TREE
                if command[0:3] == ['nix', 'derivation', 'show']:
                    return json.dumps({DRV: {}, **({'/nix/store/runtime.drv': {}} if transitive else {})})
                if command[4].startswith('.#checks'):
                    check = {'drvPath': DRV if alias else '/nix/store/runtime.drv',
                             'outputs': {'out': '/nix/store/' + '2' * 32 + '-runtime'}}
                    return json.dumps({'runtime': check}) if command[4] == '.#checks.x86_64-linux' else json.dumps(check)
                return json.dumps(DEFINITION)

            def run(command, **kwargs):
                if command[0] != 'git':
                    effects.append(command)
                return subprocess.CompletedProcess(command, 0)

            env = {'SIMIT_DEPENDENCY_ROOT': str(root), 'SIMIT_DEPENDENCY_MODE': 'import',
                   'SIMIT_DEPENDENCY_MAX_BYTES': '1024', 'SIMIT_NIX_DEPENDENCIES': '[".#input"]',
                   'SIMIT_NIX_PLAN': '[".#checks.x86_64-linux.runtime"]',
                   'SIMIT_NIX_CHECK_SYSTEMS': '["x86_64-linux"]', 'SIMIT_NIX_LIMITS': '[]',
                   'GITHUB_SHA': HEAD, 'GITHUB_EVENT_NAME': 'push', 'GITHUB_REPOSITORY': 'owner/repo',
                   'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1'}
            with patch.dict(os.environ, env), patch('subprocess.check_output', output), patch('subprocess.run', run), patch.object(Path, 'is_file', autospec=True, side_effect=lambda p: False if str(p) == 'flake.lock' else p.exists()):
                try:
                    exec(compile(PROGRAM, '<generated-dependencies>', 'exec'), {})
                except (AssertionError, RuntimeError):
                    return False, effects
            return True, effects

    def test_valid_same_run_dependency_is_imported_but_never_accepted_as_qualification(self):
        accepted, effects = self.execute()
        self.assertTrue(accepted)
        self.assertEqual(effects[0], ['nix-store', '--import'])
        self.assertEqual(effects[1], ['nix-store', '--verify-path', OUT])

    def test_other_source_run_attempt_and_checksum_reject_before_import(self):
        for field, value in [('head', 'c' * 40), ('tree', 'c' * 40), ('run', '124'),
                             ('attempt', 2), ('repository', 'other/repo'), ('complete', False),
                             ('archive_sha256', 'c' * 64), ('uncompressed_bytes', 1)]:
            with self.subTest(field=field):
                accepted, effects = self.execute((field, value))
                self.assertFalse(accepted)
                self.assertEqual(effects, [])

    def test_package_alias_of_a_check_is_rejected_before_import(self):
        accepted, effects = self.execute(alias=True)
        self.assertFalse(accepted)
        self.assertEqual(effects, [])

    def test_distinct_root_with_a_transitive_check_cannot_be_transported(self):
        accepted, effects = self.execute(transitive=True)
        self.assertFalse(accepted)
        self.assertEqual(effects, [])

    def test_authenticated_manifest_cannot_enroll_a_qualification_output_in_its_closure(self):
        accepted, effects = self.execute(('closure', [OUT, '/nix/store/' + '2' * 32 + '-runtime']))
        self.assertFalse(accepted)
        self.assertEqual(effects, [])


unittest.main()
