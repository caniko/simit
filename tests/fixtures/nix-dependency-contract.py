"""The transport's import boundary rejects provenance/bytes before store effects."""
import ast
import gzip
import hashlib
import io
import json
import os
import subprocess
import struct
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
OTHER = '/nix/store/' + '3' * 32 + '-other'
QUALIFIED = '/nix/store/' + '2' * 32 + '-runtime'


# Independent binary fixtures follow the official Nix export and NAR protocols.
def integer(value):
    return struct.pack('<Q', value)


def string(value):
    if isinstance(value, str):
        value = value.encode()
    return integer(len(value)) + value + b'\0' * (-len(value) % 8)


def regular(contents=b'input', executable=False):
    return (string('(') + string('type') + string('regular')
            + (string('executable') + string('') if executable else b'')
            + string('contents') + string(contents) + string(')'))


def symlink(target):
    return (string('(') + string('type') + string('symlink')
            + string('target') + string(target) + string(')'))


def directory(entries):
    return (string('(') + string('type') + string('directory')
            + b''.join(string('entry') + string('(') + string('name') + string(name)
                       + string('node') + child + string(')') for name, child in entries)
            + string(')'))


def record(path=OUT, node=None, references=(), deriver=DRV):
    return (integer(1) + string('nix-archive-1') + (regular() if node is None else node)
            + integer(0x4558494e) + string(path) + integer(len(references))
            + b''.join(string(reference) for reference in references)
            + string(deriver) + integer(0))


def exported(*records):
    return b''.join(records) + integer(0)


class Transport(unittest.TestCase):
    def execute(self, change=None, alias=False, transitive=False, optimized=False,
                data=None, maximum=1024, mode='import', closure=None, argument_limit=None,
                extra_environment=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / 'closure.nar.gz'
            data = exported(record()) if data is None else data
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
                if command[0:3] == ['nix', 'path-info', '--json']:
                    return json.dumps({path: {'narSize': 1} for path in closure or [OUT]})
                if command[4].startswith('.#checks'):
                    check = {'drvPath': DRV if alias else '/nix/store/runtime.drv',
                             'outputs': {'out': '/nix/store/' + '2' * 32 + '-runtime'}}
                    return json.dumps({'runtime': check}) if command[4] == '.#checks.x86_64-linux' else json.dumps(check)
                return json.dumps(DEFINITION)

            def run(command, **kwargs):
                if command[0] != 'git':
                    effects.append(command)
                if command == ['nix-store', '--import']:
                    # Actual accepted bytes must come from the fully rewound spool.
                    self.assertEqual(kwargs['stdin'].read(), data)
                    if os.name == 'posix':
                        info = os.fstat(kwargs['stdin'].fileno())
                        self.assertEqual(info.st_mode & 0o777, 0o600)
                        self.assertEqual(info.st_nlink, 0)
                stdout = json.dumps([DEFINITION]).encode() if command[0:2] == ['nix', 'build'] else None
                return subprocess.CompletedProcess(command, 0, stdout=stdout)

            class Producer:
                def __init__(self, command, **kwargs):
                    effects.append(command)
                    self.stdout = io.BytesIO(data)

                def __enter__(self):
                    return self

                def __exit__(self, *args):
                    pass

                def wait(self):
                    return 0

            env = {'SIMIT_DEPENDENCY_ROOT': str(root), 'SIMIT_DEPENDENCY_MODE': mode,
                   'SIMIT_DEPENDENCY_MAX_BYTES': str(maximum), 'SIMIT_NIX_DEPENDENCIES': '[".#input"]',
                   'SIMIT_NIX_PLAN': '[".#checks.x86_64-linux.runtime"]',
                   'SIMIT_NIX_CHECK_SYSTEMS': '["x86_64-linux"]', 'SIMIT_NIX_LIMITS': '[]',
                   'GITHUB_SHA': HEAD, 'GITHUB_EVENT_NAME': 'push', 'GITHUB_REPOSITORY': 'owner/repo',
                   'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1'}
            env.update(extra_environment or {})
            sysconf = os.sysconf
            def observed_limit(name):
                return argument_limit if argument_limit is not None and name == 'SC_ARG_MAX' else sysconf(name)
            with patch.dict(os.environ, env), patch('os.sysconf', observed_limit), patch('subprocess.check_output', output), patch('subprocess.run', run), patch('subprocess.Popen', Producer), patch.object(Path, 'is_file', autospec=True, side_effect=lambda p: False if str(p) == 'flake.lock' else p.exists()):
                try:
                    exec(compile(PROGRAM, '<generated-dependencies>', 'exec', optimize=2 if optimized else 0), {})
                except (AssertionError, RuntimeError):
                    return False, effects
            return True, effects

    def test_small_closure_exports_and_imports_under_the_worker_argument_limit(self):
        for mode in ['export', 'import']:
            for optimized in [False, True]:
                accepted, effects = self.execute(mode=mode, optimized=optimized, argument_limit=262144)
                self.assertTrue(accepted)
                operation = '--export' if mode == 'export' else '--import'
                self.assertEqual(sum(command[:2] == ['nix-store', operation] for command in effects), 1)

    def test_size_valid_export_inventory_exceeding_argv_budget_denies_before_export(self):
        closure = [OUT] + ['/nix/store/' + '0' * 32 + '-' + str(index) + 'x' * 190 for index in range(1000)]
        for optimized in [False, True]:
            accepted, effects = self.execute(mode='export', closure=closure,
                                             maximum=8 * 1024 * 1024, optimized=optimized,
                                             argument_limit=262144)
            self.assertFalse(accepted)
            self.assertFalse(any(command[0] == 'nix-store' for command in effects))

    def test_size_valid_import_inventory_exceeding_argv_budget_denies_before_import(self):
        closure = [OUT] + ['/nix/store/' + '0' * 32 + '-' + str(index) + 'x' * 190 for index in range(1000)]
        data = exported(*(record(path) for path in closure))
        for optimized in [False, True]:
            accepted, effects = self.execute(('closure', closure), data=data,
                                             maximum=8 * 1024 * 1024, optimized=optimized,
                                             argument_limit=262144)
            self.assertFalse(accepted)
            self.assertEqual(effects, [])

    def test_inherited_environment_is_charged_to_the_worker_argument_budget(self):
        for mode in ['export', 'import']:
            for optimized in [False, True]:
                accepted, effects = self.execute(mode=mode, optimized=optimized,
                                                 argument_limit=262144,
                                                 extra_environment={'TRANSPORT_AMBIENT': 'x' * 200000})
                self.assertFalse(accepted)
                self.assertFalse(any(command[0] == 'nix-store' for command in effects))

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

    def test_optimized_python_keeps_every_provenance_and_qualification_fence(self):
        for field, value in [('head', 'c' * 40), ('run', 'other'), ('attempt', 2),
                             ('archive_sha256', '0' * 64), ('archive_bytes', 0),
                             ('closure', [OUT, '/nix/store/' + '2' * 32 + '-runtime']),
                             ('uncompressed_bytes', 2 * 1024 * 1024)]:
            accepted, effects = self.execute((field, value), optimized=True)
            self.assertFalse(accepted, field)
            self.assertEqual(effects, [], field)
        for flag in ['alias', 'transitive']:
            accepted, effects = self.execute(optimized=True, **{flag: True})
            self.assertFalse(accepted, flag)
            self.assertEqual(effects, [], flag)

    def denied(self, data, change=None, **options):
        for optimized in [False, True]:
            accepted, effects = self.execute(change, optimized=optimized, data=data, **options)
            self.assertFalse(accepted)
            self.assertEqual(effects, [])

    def test_consistent_checksum_does_not_authorize_extra_or_qualification_records(self):
        for extra in [OTHER, QUALIFIED]:
            with self.subTest(path=extra):
                self.denied(exported(record(), record(extra)))

    def test_inventory_must_be_complete_unique_and_contain_evaluated_roots(self):
        for data, change in [(exported(), None), (exported(record(), record()), None),
                             (exported(record()), ('closure', [OUT, OTHER])),
                             (exported(record(OTHER)), ('closure', [OTHER])),
                             (exported(record()), ('closure', [OUT, OUT]))]:
            with self.subTest(change=change):
                self.denied(data, change)

    def test_references_and_record_paths_cannot_escape_admitted_closure(self):
        for data in [exported(record(references=[OTHER])),
                     exported(record(references=[QUALIFIED])),
                     exported(record(references=[OUT, OUT])),
                     exported(record(OUT + '/child')),
                     exported(record('/nix/store/../store/' + '0' * 32 + '-input'))]:
            self.denied(data)

    def test_framing_and_every_truncated_prefix_reject_before_any_import(self):
        valid = exported(record())
        for size in range(len(valid)):
            with self.subTest(size=size):
                self.denied(valid[:size])
        for data in [valid + b'x', valid + valid,
                     integer(2) + valid[8:],
                     valid.replace(integer(0x4558494e), integer(0x1234)),
                     valid.replace(string('input'), integer(5) + b'input\1\0\0'),
                     valid[:-16] + integer(1) + valid[-8:],
                     integer(1) + integer(2 ** 63)]:
            self.denied(data)

    def test_malformed_nar_trees_and_oversized_fields_reject_before_import(self):
        malformed = [directory([(b'b', regular()), (b'a', regular())]),
                     directory([(b'a', regular()), (b'a', regular())]),
                     directory([(b'', regular())]),
                     directory([(b'../escape', regular())]),
                     directory([(b'..', regular())]),
                     directory([(b'a\0b', regular())]),
                     symlink(b'target\0evil'),
                     regular().replace(string('contents'), string('target')),
                     string('(') + string('type') + string('regular') + string('contents') + integer(2 ** 63)]
        for node in malformed:
            with self.subTest(node=node[:96]):
                self.denied(exported(record(node=node)), maximum=65536)
        deep = regular()
        for _ in range(130):
            deep = directory([(b'a', deep)])
        self.denied(exported(record(node=deep)), maximum=65536)

    def test_valid_tree_variants_and_metadata_only_deriver_pass(self):
        for node in [regular(b''), regular(b'\0' * 17, executable=True),
                     symlink('/nix/store/../relative-target'),
                     directory([(b'a', regular()), (b'b', symlink('../a'))]),
                     directory([(b'\xff', directory([]))])]:
            accepted, effects = self.execute(data=exported(record(node=node, references=[OUT],
                                                            deriver=OTHER + '.drv')),
                                             maximum=65536)
            self.assertTrue(accepted)
            self.assertEqual(effects[0], ['nix-store', '--import'])
            self.assertEqual(sum(command == ['nix-store', '--import'] for command in effects), 1)

    def test_importer_first_invalid_tree_bounds_reject_even_after_a_valid_record(self):
        deep = regular()
        for _ in range(64):
            deep = directory([(b'a', deep)])
        for label, node in [('name256', directory([(b'a' * 256, regular())])),
                            ('target4096', symlink(b'a' * 4096)), ('depth64', deep)]:
            with self.subTest(boundary=label):
                self.denied(exported(record(), record(OTHER, node=node)),
                            ('closure', [OUT, OTHER]), maximum=65536)

    def test_importer_last_valid_tree_bounds_still_admit_the_complete_stream(self):
        deep = regular()
        for _ in range(63):
            deep = directory([(b'a', deep)])
        for label, node in [('name255', directory([(b'a' * 255, regular())])),
                            ('target4095', symlink(b'a' * 4095)), ('depth63', deep)]:
            for optimized in [False, True]:
                with self.subTest(boundary=label, optimized=optimized):
                    accepted, effects = self.execute(('closure', [OUT, OTHER]),
                        data=exported(record(), record(OTHER, node=node)),
                        optimized=optimized, maximum=65536)
                    self.assertTrue(accepted)
                    self.assertEqual(sum(command == ['nix-store', '--import'] for command in effects), 1)


class ParserBounds(unittest.TestCase):
    def setUp(self):
        # Load the exact generated parser without executing the workflow frontend.
        syntax = ast.parse(PROGRAM)
        definitions = [node for node in syntax.body
                       if isinstance(node, (ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef))]
        namespace = {}
        exec(compile(ast.Module(body=definitions, type_ignores=[]), '<exact-parser>', 'exec'), namespace)
        self.parser = namespace['ExportInventory']

    def validate(self, data, closure=None):
        return self.parser(io.BytesIO(data), len(data)).validate(closure or [OUT], [OUT], {QUALIFIED})

    def test_complete_multiple_record_closure_and_sorted_references_pass(self):
        self.validate(exported(record(OTHER), record(references=[OUT, OTHER])), [OUT, OTHER])

    def test_record_reference_and_tree_budgets_reject_at_their_bound(self):
        with patch.object(self.parser, 'MAX_PATHS', 1), self.assertRaises(RuntimeError):
            self.validate(exported(record(), record(OTHER)), [OUT, OTHER])
        with patch.object(self.parser, 'MAX_REFERENCES', 0), self.assertRaises(RuntimeError):
            self.validate(exported(record(references=[OUT])))
        with patch.object(self.parser, 'MAX_NODES', 1), self.assertRaises(RuntimeError):
            self.validate(exported(record(node=directory([(b'a', regular())]))))
        with patch.object(self.parser, 'MAX_DEPTH', 1), self.assertRaises(RuntimeError):
            self.validate(exported(record(node=directory([(b'a', regular())]))))

    def test_store_names_counts_and_metadata_fields_are_bounded(self):
        invalid = [exported(record('/nix/store/' + 'e' * 32 + '-input')),
                   exported(record('/nix/store/' + '0' * 32 + '-.-invalid')),
                   exported(record(deriver='/nix/store/../escape')),
                   exported(record()).replace(string('input'), integer(2 ** 63)),
                   exported(record(node=symlink(b'x' * 65537))),
                   exported(record(node=directory([(b'x' * 4097, regular())]))),
                   exported(record(references=[OUT, OUT])),
                   exported(record()).replace(string(DRV), integer(2 ** 63)),
                   exported(record()).replace(integer(0x4558494e) + string(OUT) + integer(0),
                                              integer(0x4558494e) + string(OUT) + integer(2 ** 63))]
        for data in invalid:
            with self.subTest(prefix=data[:96]), self.assertRaises(RuntimeError):
                self.validate(data)

    def test_file_payload_is_streamed_in_bounded_reads(self):
        class BoundedReader(io.BytesIO):
            def read(self, size=-1):
                self_test.assertLessEqual(size, 65536)
                self_test.assertGreaterEqual(size, 0)
                return super().read(size)
        self_test = self
        data = exported(record(node=regular(b'x' * (3 * 65536 + 1))))
        self.parser(BoundedReader(data), len(data)).validate([OUT], [OUT], {QUALIFIED})


unittest.main()
