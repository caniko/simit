"""Completion order, custody identity and source-head reporting contracts."""
import base64
import copy
import hashlib
import io
import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile
from urllib.parse import parse_qs, urlsplit
from pathlib import Path
from unittest.mock import patch

PROGRAM = Path(sys.argv.pop()).read_text()
HEAD = 'a' * 40
CONTEXT = 'simit/custody/pull_request/' + hashlib.sha256(b'feature').hexdigest()


class Custody(unittest.TestCase):
    optimization = 0

    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.folder = Path(self.root.name)
        self.config = {'repository': 'owner/repo', 'producer_attempt': 1,
                       'status_context': 'simit/custody', 'producers': {
                           'CI': {'workflow': '.github/workflows/ci.yaml', 'jobs': ['test'], 'artifacts': []},
                           'Nix': {'workflow': '.github/workflows/nix-builds.yaml', 'jobs': ['build'], 'artifacts': ['nix-build-0']}}}
        self.runs = [{'id': index, 'name': name, 'head_sha': HEAD, 'head_branch': 'feature',
                      'path': producer['workflow'], 'run_attempt': 1, 'event': 'pull_request',
                      'status': 'completed', 'conclusion': 'success',
                      'repository': {'full_name': 'owner/repo'}, 'head_repository': {'full_name': 'owner/repo'}}
                     for index, (name, producer) in enumerate(self.config['producers'].items(), 1)]
        self.payload = {'repository': {'full_name': 'owner/repo'}, 'workflow_run': copy.deepcopy(self.runs[0])}
        self.previous = []
        self.effects = []
        self.calls = []
        self.job_logs = {10: b'CI provider log\n', 20: b'Nix provider log\n'}
        blob = b'name: fixture\non: push\n'
        self.source = {'type': 'file', 'encoding': 'base64', 'content': base64.b64encode(blob).decode(),
                       'sha': hashlib.sha1(b'blob ' + str(len(blob)).encode() + b'\0' + blob).hexdigest()}
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            archive.writestr('retained.txt', 'ordinary untrusted log data')
        self.zip = stream.getvalue()
        self.namespace = {'__name__': 'contract'}
        exec(compile(PROGRAM, '<generated-custody>', 'exec', optimize=self.optimization), self.namespace)
        self.namespace['api'] = self.api
        self.environment = patch.dict(os.environ, {'RUNNER_ENVIRONMENT': 'github-hosted',
            'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_SHA': 'c' * 40, 'GITHUB_RUN_ID': '900',
            'GITHUB_RUN_ATTEMPT': '1', 'SIMIT_CUSTODY_UPLOAD_OUTCOME': 'success',
            'SIMIT_CUSTODY_ARTIFACT_ID': '88'})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def api(self, repository, endpoint, **limits):
        self.assertEqual(repository, 'owner/repo')
        self.calls.append(endpoint)
        if endpoint.startswith('actions/runs?'):
            return json.dumps({'total_count': len(self.runs), 'workflow_runs': self.runs}).encode()
        if endpoint.startswith('git/commits/'):
            return json.dumps({'sha': HEAD, 'tree': {'sha': 'b' * 40}}).encode()
        if endpoint.startswith('contents/'):
            return json.dumps(self.source).encode()
        if endpoint.startswith('commits/') and '/status?' in endpoint:
            page = int(parse_qs(urlsplit(endpoint).query)['page'][0])
            return json.dumps({'total_count': len(self.previous),
                               'statuses': self.previous[(page-1)*100:page*100]}).encode()
        if endpoint.startswith('actions/jobs/'):
            return self.job_logs.get(int(endpoint.split('/')[2]), b'')
        if endpoint.startswith('actions/artifacts/') or endpoint.endswith('/logs'):
            return self.zip
        if '/jobs?' in endpoint:
            run_id = int(endpoint.split('/')[2])
            name = 'test' if run_id == 1 else 'build'
            return json.dumps({'total_count': 1, 'jobs': [{'id': run_id * 10, 'name': name, 'run_id': run_id,
                'head_sha': HEAD, 'run_attempt': 1, 'status': 'completed', 'conclusion': 'success'}]}).encode()
        if '/artifacts?' in endpoint:
            artifacts = [] if '/1/' in endpoint else [{'id': 20, 'name': 'nix-build-0', 'expired': False, 'size_in_bytes': len(self.zip),
                'workflow_run': {'id': 2, 'head_sha': HEAD}, 'digest': 'sha256:' + hashlib.sha256(self.zip).hexdigest()}]
            return json.dumps({'total_count': len(artifacts), 'artifacts': artifacts}).encode()
        if endpoint.startswith('actions/runs/'):
            return json.dumps(next(run for run in self.runs if run['id'] == int(endpoint.split('/')[2]))).encode()
        raise AssertionError('Unexpected endpoint: ' + endpoint)

    def captured(self):
        receipt = self.namespace['capture'](self.config, self.payload, self.folder)
        self.namespace['write'](self.folder / 'receipt.json', receipt)
        return receipt

    def main_capture(self):
        event = self.folder / 'input-event.json'
        event.write_text(json.dumps(self.payload))
        with patch.dict(os.environ, {'SIMIT_CUSTODY_CONFIG': json.dumps(self.config),
                'SIMIT_CUSTODY_ROOT': str(self.folder), 'SIMIT_CUSTODY_PHASE': 'capture',
                'GITHUB_EVENT_PATH': str(event)}):
            return self.namespace['main']()

    def report(self):
        def effect(command, **kwargs):
            self.effects.append((command, json.loads(kwargs['input'])))
            return subprocess.CompletedProcess(command, 0)
        with patch('subprocess.run', effect):
            return self.namespace['report'](self.config, self.folder)

    def test_either_completion_stays_pending_until_all_originals_complete(self):
        self.runs[1]['status'] = 'in_progress'
        receipt = self.captured()
        self.assertEqual(receipt['state'], 'waiting')
        self.assertFalse(receipt['capture_complete'])
        self.assertFalse(any(endpoint.endswith('/logs') for endpoint in self.calls))
        self.assertEqual(self.report(), 0)
        self.assertEqual(self.effects[0][1]['state'], 'pending')

    def test_complete_custody_reports_source_head_not_default_branch_and_is_idempotent(self):
        receipt = self.captured()
        self.assertEqual(receipt['state'], 'ready')
        self.assertTrue(receipt['capture_complete'])
        self.assertFalse(receipt['observation_is_execution'])
        self.assertFalse(receipt['qualified'])
        self.assertEqual(self.report(), 0)
        command, body = self.effects[0]
        self.assertIn('repos/owner/repo/statuses/' + HEAD, command)
        self.assertNotIn('repos/owner/repo/statuses/' + 'c' * 40, command)
        self.assertEqual(body['context'], CONTEXT)
        self.assertEqual(body['state'], 'success')
        self.previous = [body]
        self.assertEqual(self.report(), 0)
        self.assertEqual(len(self.effects), 1)

    def test_failed_producer_and_failed_custody_upload_cannot_report_success(self):
        self.runs[1]['conclusion'] = 'failure'
        self.assertEqual(self.captured()['state'], 'failed')
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[0][1]['state'], 'failure')

    def test_upload_is_required_even_after_every_producer_succeeds(self):
        self.captured()
        with patch.dict(os.environ, {'SIMIT_CUSTODY_UPLOAD_OUTCOME': 'failure'}):
            self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[0][1]['state'], 'failure')

    def test_duplicate_originals_and_nonoriginal_attempts_reject(self):
        self.runs.append({**self.runs[1], 'id': 3})
        with self.assertRaises(RuntimeError):
            self.captured()
        self.runs.pop()
        self.runs[1]['run_attempt'] = 2
        with self.assertRaises(RuntimeError):
            self.captured()
        self.assertEqual(self.effects, [])

    def test_nonoriginal_completion_revokes_an_existing_success_through_main(self):
        self.previous = [{'context': CONTEXT, 'state': 'success',
                          'description': 'old admitted observation'}]
        self.runs[0]['run_attempt'] = 2
        self.payload['workflow_run'] = copy.deepcopy(self.runs[0])
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_rerun_after_capture_and_before_reporting_refuses_stale_success(self):
        self.captured()
        self.runs[1]['run_attempt'] = 2
        self.runs[1]['status'] = 'in_progress'
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_trigger_detail_failure_retains_a_safe_failure_reporting_target(self):
        original_api = self.namespace['api']
        def unavailable(repository, endpoint, **limits):
            if endpoint == 'actions/runs/1':
                raise RuntimeError('provider detail unavailable')
            return original_api(repository, endpoint, **limits)
        self.namespace['api'] = unavailable
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_fork_sibling_with_identical_sha_branch_and_workflow_is_rejected(self):
        self.runs[1]['head_repository'] = {'full_name': 'fork/repo'}
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_missing_enrolled_job_log_rejects_even_with_nonempty_run_archive(self):
        self.job_logs.pop(20)
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_status_beyond_page_one_remains_idempotent(self):
        self.captured()
        self.assertEqual(self.report(), 0)
        status = self.effects[-1][1]
        self.previous = [{'context': f'unrelated-{i}', 'state': 'success', 'description': 'other'}
                         for i in range(120)] + [status]
        self.assertEqual(self.report(), 0)
        self.assertEqual(len(self.effects), 1)
        self.assertTrue(any('/status?per_page=100&page=2' in endpoint for endpoint in self.calls))

    def test_failure_is_published_when_status_inventory_exceeds_its_bound(self):
        self.previous = [{'context': CONTEXT, 'state': 'success',
                          'description': 'old admitted observation'}]
        self.runs[0]['run_attempt'] = 2
        self.payload['workflow_run'] = copy.deepcopy(self.runs[0])
        with self.assertRaises(RuntimeError):
            self.main_capture()
        original_api = self.namespace['api']
        def oversized(repository, endpoint, **limits):
            if endpoint.startswith('commits/'):
                return json.dumps({'total_count': 2001, 'statuses': self.previous}).encode()
            return original_api(repository, endpoint, **limits)
        self.namespace['api'] = oversized
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertTrue((self.folder / 'report-failure.json').is_file())

    def test_unavailable_status_lookup_cannot_suppress_fail_closed_publication(self):
        self.captured()
        original_api = self.namespace['api']
        def unavailable(repository, endpoint, **limits):
            if endpoint.startswith('commits/'):
                raise RuntimeError('status inventory unavailable')
            return original_api(repository, endpoint, **limits)
        self.namespace['api'] = unavailable
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        diagnostic = json.loads((self.folder / 'report-failure.json').read_text())
        self.assertIn('status inventory unavailable', diagnostic['status_lookup_error'])

    def test_duplicate_archive_paths_reject_without_extraction(self):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            archive.writestr('same', 'first')
            archive.writestr('same', 'second')
        with self.assertRaises(RuntimeError):
            self.namespace['zip_members'](stream.getvalue())
        self.assertEqual(list(self.folder.iterdir()), [])

    def test_repository_case_differences_preserve_capture_and_reporting_identity(self):
        self.config['repository'] = 'Owner/Repo'
        self.payload['repository']['full_name'] = 'OWNER/Repo'
        for run in self.runs:
            run['repository']['full_name'] = 'Owner/REPO'
            run['head_repository']['full_name'] = 'OWNER/REPO'
        self.assertEqual(self.captured()['state'], 'ready')
        self.assertEqual(self.report(), 0)
        self.assertIn('repos/owner/repo/statuses/' + HEAD, self.effects[-1][0])

    def test_same_sha_event_on_different_branches_has_distinct_status_contexts(self):
        self.captured()
        self.report()
        first = self.effects[-1][1]
        self.previous = [first]
        # A later observation has the same source SHA and event, but another branch.
        receipt = json.loads((self.folder / 'receipt.json').read_text())
        receipt['branch'] = 'different/branch'
        for run in self.runs:
            run['head_branch'] = receipt['branch']
        self.namespace['write'](self.folder / 'receipt.json', receipt)
        self.report()
        self.assertEqual(len(self.effects), 2)
        self.assertNotEqual(first['context'], self.effects[-1][1]['context'])

    def test_total_retained_log_budget_rejects_before_later_downloads_and_reports_failure(self):
        # Each input fits the old per-item cap; their aggregate does not.
        self.job_logs[10] = b'x' * 1500
        self.job_logs[20] = b'y' * 1500
        self.namespace['MAX_CAPTURE_BYTES'] = 5000
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertFalse(any(endpoint.startswith('actions/artifacts/') for endpoint in self.calls))

    def test_aggregate_archive_expansion_and_member_count_are_bounded(self):
        # Reusing a small compressed archive twice must charge both expansions.
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr('retained.txt', b'x' * 3000)
        self.zip = stream.getvalue()
        self.namespace['MAX_CAPTURE_BYTES'] = 20000
        self.namespace['MAX_EXPANDED_BYTES'] = 5000
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_capture_request_and_retained_file_counts_are_bounded(self):
        self.namespace['MAX_CAPTURE_REQUESTS'] = 3
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_archive_member_and_retained_file_limits_stop_capture(self):
        self.namespace['MAX_ARCHIVE_MEMBERS'] = 1
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_retained_file_inventory_rejects_before_later_archive_download(self):
        self.namespace['MAX_CAPTURE_FILES'] = 2
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertFalse(any(endpoint.endswith('/logs') for endpoint in self.calls))

    def test_capture_deadline_is_shared_across_requests(self):
        now = [100.0]
        original_api = self.namespace['api']
        def slow(repository, endpoint, **limits):
            raw = original_api(repository, endpoint, **limits)
            now[0] += 100
            return raw
        self.namespace['api'] = slow
        self.namespace['MAX_CAPTURE_SECONDS'] = 300
        with patch.object(self.namespace['time'], 'monotonic', lambda: now[0]):
            with self.assertRaises(RuntimeError):
                self.main_capture()
        self.assertEqual(len(self.calls), 3)
        self.assertEqual(self.report(), 1)

    def test_actual_central_directory_bound_rejects_before_eager_zip_construction(self):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            for i in range(4):
                archive.writestr(str(i), '')
        raw = bytearray(stream.getvalue())
        footer = raw.rfind(b'PK\x05\x06')
        # Python parses every directory record even when the EOCD count lies.
        raw[footer + 8:footer + 12] = b'\x01\0\x01\0'
        self.namespace['MAX_ARCHIVE_MEMBERS'] = 2
        construct = zipfile.ZipFile
        with patch('zipfile.ZipFile', wraps=construct) as eager:
            with self.assertRaises(RuntimeError):
                self.namespace['zip_members'](bytes(raw))
            eager.assert_not_called()

    def test_expired_archive_deadline_rejects_before_eager_zip_construction(self):
        budget = self.namespace['CaptureBudget']()
        budget.deadline = 0
        construct = zipfile.ZipFile
        with patch('zipfile.ZipFile', wraps=construct) as eager:
            with self.assertRaises(RuntimeError):
                self.namespace['zip_members'](self.zip, budget)
            eager.assert_not_called()

    def rejected_before_construction(self, raw, budget=None):
        construct = zipfile.ZipFile
        with patch('zipfile.ZipFile', wraps=construct) as eager:
            with self.assertRaises((RuntimeError, zipfile.BadZipFile)):
                self.namespace['zip_members'](raw, budget)
            eager.assert_not_called()

    def test_directory_byte_and_expansion_caps_reject_before_construction(self):
        self.namespace['MAX_DIRECTORY_BYTES'] = 1
        self.rejected_before_construction(self.zip)
        self.namespace['MAX_DIRECTORY_BYTES'] = 16 * 1024 * 1024
        self.namespace['MAX_EXPANDED_BYTES'] = 1
        self.rejected_before_construction(self.zip)

    def test_previous_archives_charge_member_budget_before_construction(self):
        budget = self.namespace['CaptureBudget']()
        self.namespace['MAX_ARCHIVE_MEMBERS'] = 1
        self.namespace['zip_members'](self.zip, budget)
        self.rejected_before_construction(self.zip, budget)

    def test_truncated_and_malformed_directory_records_reject_before_construction(self):
        directory = self.zip.index(b'PK\x01\x02')
        footer = self.zip.rfind(b'PK\x05\x06')
        variants = []
        bad_signature = bytearray(self.zip)
        bad_signature[directory] = 0
        variants.append(bytes(bad_signature))
        bad_field_length = bytearray(self.zip)
        struct.pack_into('<H', bad_field_length, directory + 28, 65535)
        variants.append(bytes(bad_field_length))
        truncated = bytearray(self.zip)
        struct.pack_into('<I', truncated, footer + 12, 45)
        struct.pack_into('<I', truncated, footer + 16, footer - 45)
        variants.append(bytes(truncated))
        variants.append(self.zip[:footer])
        for raw in variants:
            with self.subTest(raw=raw):
                self.rejected_before_construction(raw)

    def test_directory_scan_obeys_original_deadline_before_construction(self):
        budget = self.namespace['CaptureBudget']()
        budget.deadline = 10
        ticks = iter([0, 1, 11])
        with patch.object(self.namespace['time'], 'monotonic', lambda: next(ticks)):
            self.rejected_before_construction(self.zip, budget)

    def test_stored_deflated_comment_and_concatenated_archives_preserve_hashes(self):
        payload = b'provider log\n' * 10000
        for method in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w', compression=method) as archive:
                archive.comment = b'bounded provider comment'
                archive.writestr('logs/', b'')
                archive.writestr('logs/job.txt', payload)
            raw = stream.getvalue()
            for prefix in (b'', b'provider-prefix'):
                with self.subTest(method=method, prefix=prefix):
                    self.assertEqual(self.namespace['zip_members'](prefix + raw),
                                     {'logs/job.txt': hashlib.sha256(payload).hexdigest()})
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w'):
            pass
        self.assertEqual(self.namespace['zip_members'](stream.getvalue()), {})

    def test_zip64_directory_and_member_sizes_pass_or_reject_before_construction(self):
        stream = io.BytesIO()
        payload = b'ZIP64 provider bytes'
        # Force real standard-library ZIP64 member extras and end records while
        # keeping the fixture tiny; restore the interpreter constant immediately.
        with patch.object(zipfile, 'ZIP64_LIMIT', 1):
            with zipfile.ZipFile(stream, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
                archive.writestr('job.txt', payload)
        raw = stream.getvalue()
        self.assertIn(b'PK\x06\x06', raw)
        self.assertEqual(self.namespace['zip_members'](raw),
                         {'job.txt': hashlib.sha256(payload).hexdigest()})
        self.assertEqual(self.namespace['zip_members'](b'prefix' + raw),
                         {'job.txt': hashlib.sha256(payload).hexdigest()})
        self.namespace['MAX_EXPANDED_BYTES'] = len(payload) - 1
        self.rejected_before_construction(raw)
        self.namespace['MAX_EXPANDED_BYTES'] = 512 * 1024 * 1024
        directory = raw.index(b'PK\x01\x02')
        extra = directory + 46 + len(b'job.txt')
        truncated = bytearray(raw)
        struct.pack_into('<H', truncated, extra + 2, 4)
        self.rejected_before_construction(bytes(truncated))

    def test_unbounded_compression_methods_reject_before_construction(self):
        # LZMA's ZIP decoder accepts attacker-controlled dictionary allocation;
        # custody admits the provider's stored/deflated profile instead.
        raw = bytearray(self.zip)
        directory = raw.index(b'PK\x01\x02')
        struct.pack_into('<H', raw, directory + 10, zipfile.ZIP_LZMA)
        self.rejected_before_construction(bytes(raw))


class Downloads(unittest.TestCase):
    optimization = 0

    def setUp(self):
        self.namespace = {'__name__': 'contract'}
        exec(compile(PROGRAM, '<generated-custody>', 'exec', optimize=self.optimization), self.namespace)

    def download(self, source, maximum, timeout=5):
        create = subprocess.Popen
        children = []
        def fixture(command, **kwargs):
            child = create([sys.executable, '-c', source], **kwargs)
            children.append(child)
            return child
        try:
            with patch('subprocess.Popen', fixture):
                return self.namespace['api']('owner/repo', 'fixture', maximum=maximum, timeout=timeout)
        finally:
            self.assertEqual(len(children), 1)
            self.assertIsNotNone(children[0].poll())
            self.assertFalse(any(thread.name.startswith('custody-download-') and thread.is_alive()
                                 for thread in self.namespace['threading'].enumerate()))

    def test_download_exact_boundary_passes_and_first_excess_reaps_child(self):
        source = "import sys; sys.stdout.buffer.write(b'x' * 65537)"
        self.assertEqual(len(self.download(source, maximum=65537)), 65537)
        with self.assertRaises(RuntimeError):
            self.download(source, maximum=65536)

    def test_stalled_download_and_oversized_stderr_reap_child(self):
        with self.assertRaises(RuntimeError):
            self.download('import time; time.sleep(60)', maximum=1, timeout=0.05)
        with self.assertRaises(RuntimeError):
            self.download("import sys; sys.stderr.buffer.write(b'x' * 65537)", maximum=1)

    def test_failed_provider_download_cannot_return_partial_success(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.download("import sys; sys.stdout.buffer.write(b'x'); sys.exit(1)", maximum=1)


class OptimizedCustody(Custody):
    optimization = 2


class OptimizedDownloads(Downloads):
    optimization = 2


unittest.main()
