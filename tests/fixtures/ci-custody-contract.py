"""Completion order, custody identity and source-head reporting contracts."""
import base64
import copy
import hashlib
import io
import json
import os
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


class Custody(unittest.TestCase):
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
        exec(compile(PROGRAM, '<generated-custody>', 'exec'), self.namespace)
        self.namespace['api'] = self.api
        self.environment = patch.dict(os.environ, {'RUNNER_ENVIRONMENT': 'github-hosted',
            'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_SHA': 'c' * 40, 'GITHUB_RUN_ID': '900',
            'GITHUB_RUN_ATTEMPT': '1', 'SIMIT_CUSTODY_UPLOAD_OUTCOME': 'success',
            'SIMIT_CUSTODY_ARTIFACT_ID': '88'})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def api(self, repository, endpoint):
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
        self.assertEqual(body['context'], 'simit/custody/pull_request')
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
        with self.assertRaises(AssertionError):
            self.captured()
        self.runs.pop()
        self.runs[1]['run_attempt'] = 2
        with self.assertRaises(AssertionError):
            self.captured()
        self.assertEqual(self.effects, [])

    def test_nonoriginal_completion_revokes_an_existing_success_through_main(self):
        self.previous = [{'context': 'simit/custody/pull_request', 'state': 'success',
                          'description': 'old admitted observation'}]
        self.runs[0]['run_attempt'] = 2
        self.payload['workflow_run'] = copy.deepcopy(self.runs[0])
        with self.assertRaises(AssertionError):
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
        def unavailable(repository, endpoint):
            if endpoint == 'actions/runs/1':
                raise RuntimeError('provider detail unavailable')
            return original_api(repository, endpoint)
        self.namespace['api'] = unavailable
        with self.assertRaises(RuntimeError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_fork_sibling_with_identical_sha_branch_and_workflow_is_rejected(self):
        self.runs[1]['head_repository'] = {'full_name': 'fork/repo'}
        with self.assertRaises(AssertionError):
            self.main_capture()
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_missing_enrolled_job_log_rejects_even_with_nonempty_run_archive(self):
        self.job_logs.pop(20)
        with self.assertRaises(AssertionError):
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
        self.previous = [{'context': 'simit/custody/pull_request', 'state': 'success',
                          'description': 'old admitted observation'}]
        self.runs[0]['run_attempt'] = 2
        self.payload['workflow_run'] = copy.deepcopy(self.runs[0])
        with self.assertRaises(AssertionError):
            self.main_capture()
        original_api = self.namespace['api']
        def oversized(repository, endpoint):
            if endpoint.startswith('commits/'):
                return json.dumps({'total_count': 2001, 'statuses': self.previous}).encode()
            return original_api(repository, endpoint)
        self.namespace['api'] = oversized
        self.assertEqual(self.report(), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertTrue((self.folder / 'report-failure.json').is_file())

    def test_unavailable_status_lookup_cannot_suppress_fail_closed_publication(self):
        self.captured()
        original_api = self.namespace['api']
        def unavailable(repository, endpoint):
            if endpoint.startswith('commits/'):
                raise RuntimeError('status inventory unavailable')
            return original_api(repository, endpoint)
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
        with self.assertRaises(AssertionError):
            self.namespace['zip_members'](stream.getvalue())
        self.assertEqual(list(self.folder.iterdir()), [])


unittest.main()
