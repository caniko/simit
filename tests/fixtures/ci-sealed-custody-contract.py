"""Prospective sealed-custody contracts; the legacy fixture stays independent."""
from pathlib import Path
import sys

support = {'__name__': 'legacy_support'}
exec(compile(Path('tests/fixtures/ci-custody-contract.py').read_text().rsplit('unittest.main()', 1)[0],
             '<legacy-support>', 'exec'), support)
globals().update({key: value for key, value in support.items() if key != '__name__'
                 and not (isinstance(value, type) and issubclass(value, support['unittest'].TestCase))})


class Sealed(unittest.TestCase):
    optimization = 0
    api = support['Custody'].api

    def setUp(self):
        support['Custody'].setUp(self)
        self.config['protocol'] = 'sealed-v2'
        self.payload['repository']['default_branch'] = 'trunk'
        self.jobs = {}
        for run in self.runs:
            name = 'test' if run['id'] == 1 else 'build'
            self.jobs[run['id']] = [{'id': run['id'] * 10, 'name': name, 'run_id': run['id'],
                'head_sha': HEAD, 'run_attempt': 1, 'status': 'completed', 'conclusion': 'success',
                'steps': [{'number': 1, 'name': 'Set up job', 'status': 'completed', 'conclusion': 'success'},
                          {'number': 2, 'name': 'Test', 'status': 'completed', 'conclusion': 'success'},
                          {'number': 3, 'name': 'Complete job', 'status': 'completed', 'conclusion': 'success'}]}]
        self.zips = {}
        self.producer_artifacts = [{'id': 20, 'name': 'nix-build-0', 'expired': False, 'size_in_bytes': len(self.zip),
            'workflow_run': {'id': 2, 'head_sha': HEAD}, 'digest': 'sha256:' + hashlib.sha256(self.zip).hexdigest()}]
        for run_id, jobs in self.jobs.items():
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as archive:
                for step in jobs[0]['steps']:
                    archive.writestr(f"{jobs[0]['name']}/{step['number']}_{step['name']}.txt", b'provider log\n')
            self.zips[run_id] = stream.getvalue()
        original = self.namespace['api']
        def provider(repository, endpoint, **limits):
            if '/artifacts?' in endpoint:
                rows = [] if '/1/' in endpoint else self.producer_artifacts
                return json.dumps({'total_count': len(rows), 'artifacts': rows}).encode()
            if '/jobs?' in endpoint:
                jobs = self.jobs[int(endpoint.split('/')[2])]
                return json.dumps({'total_count': len(jobs), 'jobs': jobs}).encode()
            if endpoint.endswith('/logs') and endpoint.startswith('actions/runs/'):
                return self.zips[int(endpoint.split('/')[2])]
            return original(repository, endpoint, **limits)
        self.namespace['api'] = provider
        self.native_provider = provider

    def seal_original(self, run_id, artifact_id, observer_id):
        folder = self.folder / f'capsule-{run_id}'
        folder.mkdir()
        payload = copy.deepcopy(self.payload)
        payload['workflow_run'] = copy.deepcopy(self.runs[run_id - 1])
        with patch.dict(os.environ, {'GITHUB_RUN_ID': str(observer_id)}):
            self.namespace['sealed_capture'](self.config, payload, folder)
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            for path in folder.rglob('*'):
                if path.is_file():
                    archive.writestr(str(path.relative_to(folder)), path.read_bytes())
        raw = stream.getvalue()
        manifest = json.loads((folder / 'manifest.json').read_bytes())
        artifact = {'id': artifact_id, 'name': manifest['key'], 'expired': False,
                    'size_in_bytes': len(raw), 'digest': 'sha256:' + hashlib.sha256(raw).hexdigest(),
                    'workflow_run': {'id': observer_id, 'head_sha': 'c' * 40}}
        return artifact, raw

    def install_capsule_provider(self, capsules):
        self.capsules = {artifact['id']: (artifact, raw) for artifact, raw in capsules}
        self.observers = {}
        for artifact, _ in capsules:
            observer_id = artifact['workflow_run']['id']
            run = copy.deepcopy(self.runs[0])
            run.update(id=observer_id, node_id=f'observer-{observer_id}', check_suite_id=observer_id * 100,
                       head_sha='c' * 40, head_branch='trunk', event='workflow_run',
                       name='Retain completed producer custody', path='.github/workflows/producer-custody.yaml')
            self.observers[observer_id] = run
            node = copy.deepcopy(self.workflow_files['workflow-node-1']['data']['node'])
            node.update(id=run['node_id'], databaseId=observer_id, event='workflow_run')
            node['checkSuite']['databaseId'] = run['check_suite_id']
            node['file'].update(path=run['path'], repositoryFileUrl=f"https://github.com/owner/repo/blob/{'c' * 40}/{run['path']}",
                                run={'id': run['node_id'], 'databaseId': observer_id, 'runAttempt': 1})
            self.workflow_files[run['node_id']] = {'data': {'node': node}}
        original = self.native_provider
        def provider(repository, endpoint, **limits):
            if endpoint.startswith('actions/artifacts?'):
                name = parse_qs(urlsplit(endpoint).query)['name'][0]
                rows = [artifact for artifact, _ in self.capsules.values() if artifact['name'] == name]
                return json.dumps({'total_count': len(rows), 'artifacts': rows}).encode()
            if endpoint.startswith('actions/artifacts/') and int(endpoint.split('/')[2]) != 20:
                artifact, raw = self.capsules[int(endpoint.split('/')[2])]
                return raw if endpoint.endswith('/zip') else json.dumps(artifact).encode()
            if endpoint.startswith('actions/runs/'):
                run_id = int(endpoint.split('/')[2])
                if run_id in self.observers:
                    if '/jobs?' in endpoint:
                        return json.dumps({'total_count': 1, 'jobs': [{'id': run_id * 10, 'name': 'seal',
                            'run_id': run_id, 'head_sha': 'c' * 40, 'run_attempt': 1,
                            'status': 'completed', 'conclusion': 'success'}]}).encode()
                    return json.dumps(self.observers[run_id]).encode()
            return original(repository, endpoint, **limits)
        self.namespace['api'] = provider

    def aggregate(self):
        root = self.folder / 'aggregate'
        root.mkdir(exist_ok=True)
        return self.namespace['sealed_aggregate'](self.config, self.payload, root)

    def test_independent_aggregate_reuses_exact_capsules_without_ephemeral_log_downloads(self):
        capsules = [self.seal_original(1, 88, 900), self.seal_original(2, 89, 901)]
        self.install_capsule_provider(capsules)
        with patch.dict(os.environ, {'SIMIT_CUSTODY_CAPSULE_ID': '88',
                'SIMIT_CUSTODY_CAPSULE_DIGEST': capsules[0][0]['digest'], 'SIMIT_CUSTODY_SEAL_OUTCOME': 'success'}):
            self.calls.clear()
            receipt = self.aggregate()
        self.assertEqual(receipt['state'], 'ready')
        self.assertEqual(receipt['schema'], 'simit.producer-custody.v2')
        self.assertEqual([row['capsule']['id'] for row in receipt['producers']], [88, 89])
        self.assertFalse(any(call.endswith('/logs') for call in self.calls))

    def test_missing_sibling_capsule_stays_pending(self):
        capsule = self.seal_original(1, 88, 900)
        self.install_capsule_provider([capsule])
        with patch.dict(os.environ, {'SIMIT_CUSTODY_CAPSULE_ID': '88',
                'SIMIT_CUSTODY_CAPSULE_DIGEST': capsule[0]['digest'], 'SIMIT_CUSTODY_SEAL_OUTCOME': 'success'}):
            receipt = self.aggregate()
        self.assertEqual(receipt['state'], 'waiting')
        self.assertFalse(receipt['capture_complete'])

    def prepared_report(self):
        capsules = [self.seal_original(1, 88, 900), self.seal_original(2, 89, 901)]
        self.install_capsule_provider(capsules)
        os.environ.update(SIMIT_CUSTODY_CAPSULE_ID='88', SIMIT_CUSTODY_CAPSULE_DIGEST=capsules[0][0]['digest'],
                          SIMIT_CUSTODY_SEAL_OUTCOME='success')
        receipt = self.aggregate()
        root = self.folder / 'aggregate'
        self.upload_aggregate(root)
        return receipt, root

    def upload_aggregate(self, root):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            archive.writestr('receipt.json', (root / 'receipt.json').read_bytes())
        raw = stream.getvalue()
        artifact = {'id': 90, 'name': 'producer-custody-v2-900-1', 'expired': False,
                    'size_in_bytes': len(raw), 'digest': 'sha256:' + hashlib.sha256(raw).hexdigest(),
                    'workflow_run': {'id': 900, 'head_sha': 'c' * 40}}
        self.capsules[90] = (artifact, raw)
        os.environ.update(SIMIT_CUSTODY_ARTIFACT_ID='90', SIMIT_CUSTODY_ARTIFACT_DIGEST=artifact['digest'])

    def sealed_report(self, root):
        def effect(command, **kwargs):
            self.effects.append((command, json.loads(kwargs['input'])))
            return subprocess.CompletedProcess(command, 0)
        with patch('subprocess.run', effect):
            return self.namespace['sealed_report'](self.config, self.payload, root)

    def test_report_requires_uploaded_aggregate_and_fresh_exact_originals(self):
        _, root = self.prepared_report()
        self.assertEqual(self.sealed_report(root), 0)
        self.assertEqual(self.effects[-1][1]['state'], 'success')
        self.assertEqual(self.effects[-1][1]['context'], CONTEXT)
        self.runs[0]['run_attempt'] = 2
        self.assertEqual(self.sealed_report(root), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertEqual(self.effects[-1][1]['context'], CONTEXT)

    def test_first_waiting_is_pending_but_missing_previously_accepted_set_revokes(self):
        capsule = self.seal_original(1, 88, 900)
        self.install_capsule_provider([capsule])
        os.environ.update(SIMIT_CUSTODY_CAPSULE_ID='88', SIMIT_CUSTODY_CAPSULE_DIGEST=capsule[0]['digest'],
                          SIMIT_CUSTODY_SEAL_OUTCOME='success')
        self.assertEqual(self.aggregate()['state'], 'waiting')
        root = self.folder / 'aggregate'
        self.upload_aggregate(root)
        self.assertEqual(self.sealed_report(root), 0)
        self.assertEqual(self.effects[-1][1]['state'], 'pending')
        self.previous = [{'context': CONTEXT, 'state': 'success', 'description': 'Earlier accepted set'}]
        self.assertEqual(self.sealed_report(root), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertEqual(self.effects[-1][1]['context'], CONTEXT)

    def test_post_upload_capsule_deletion_expiry_and_overwrite_never_substitute(self):
        for mutation in ['delete', 'expire', 'overwrite', 'digest', 'duplicate']:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                original = self.folder
                self.folder = Path(directory)
                _, root = self.prepared_report()
                artifact, raw = self.capsules[89]
                if mutation == 'delete':
                    del self.capsules[89]
                elif mutation == 'expire':
                    artifact['expired'] = True
                elif mutation == 'overwrite':
                    del self.capsules[89]
                    self.capsules[99] = (dict(artifact, id=99), raw)
                elif mutation == 'digest':
                    artifact['digest'] = 'sha256:' + '0' * 64
                else:
                    self.capsules[99] = (dict(artifact, id=99), raw)
                self.assertEqual(self.sealed_report(root), 1)
                self.assertEqual(self.effects[-1][1]['state'], 'failure')
                self.folder = original

    def test_custody_bytes_cannot_be_laundered_through_a_producer_owned_artifact(self):
        _, root = self.prepared_report()
        observer = self.observers[901]
        observer['event'] = 'pull_request'
        self.assertEqual(self.sealed_report(root), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')

    def test_unuploaded_or_replaced_aggregate_never_reports_success(self):
        for mutation in ['upload-failure', 'wrong-id', 'wrong-digest', 'local-receipt']:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                original = self.folder
                self.folder = Path(directory)
                receipt, root = self.prepared_report()
                if mutation == 'upload-failure':
                    os.environ['SIMIT_CUSTODY_UPLOAD_OUTCOME'] = 'failure'
                elif mutation == 'wrong-id':
                    os.environ['SIMIT_CUSTODY_ARTIFACT_ID'] = '89'
                elif mutation == 'wrong-digest':
                    os.environ['SIMIT_CUSTODY_ARTIFACT_DIGEST'] = '0' * 64
                else:
                    receipt['producers'][0]['logs_sha256'] = '0' * 64
                    (root / 'receipt.json').write_text(json.dumps(receipt))
                self.assertEqual(self.sealed_report(root), 1)
                self.assertEqual(self.effects[-1][1]['state'], 'failure')
                os.environ['SIMIT_CUSTODY_UPLOAD_OUTCOME'] = 'success'
                self.folder = original

    def test_both_completion_orders_capture_independently_then_aggregate(self):
        for first, second in [(1, 2), (2, 1)]:
            with self.subTest(first=first), tempfile.TemporaryDirectory() as directory:
                original = self.folder
                self.folder = Path(directory)
                self.runs[second - 1]['status'] = 'in_progress'
                self.payload['workflow_run'] = copy.deepcopy(self.runs[first - 1])
                one = self.seal_original(first, 88, 900)
                self.install_capsule_provider([one])
                os.environ.update(SIMIT_CUSTODY_CAPSULE_ID='88', SIMIT_CUSTODY_CAPSULE_DIGEST=one[0]['digest'],
                                  SIMIT_CUSTODY_SEAL_OUTCOME='success')
                self.assertEqual(self.aggregate()['state'], 'waiting')
                self.runs[second - 1]['status'] = 'completed'
                two = self.seal_original(second, 89, 901)
                self.install_capsule_provider([one, two])
                self.assertEqual(self.aggregate()['state'], 'ready')
                self.folder = original

    def test_seal_survives_replaced_aggregate_without_crediting_failed_observation(self):
        _, root = self.prepared_report()
        self.observers[901]['conclusion'] = 'cancelled'
        self.assertEqual(self.sealed_report(root), 0)
        self.assertEqual(self.observers[901]['conclusion'], 'cancelled')
        self.observers[901]['run_attempt'] = 2
        self.assertEqual(self.sealed_report(root), 1)

    def test_latest_native_snapshot_jobs_artifacts_and_workflow_proof_revoke(self):
        for mutation in ['base', 'job', 'artifact', 'workflow', 'collector-workflow', 'collector-branch', 'collector-attempt', 'duplicate', 'qualifier', 'name']:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                original = self.folder
                self.folder = Path(directory)
                runs = copy.deepcopy(self.runs)
                jobs = copy.deepcopy(self.jobs)
                files = copy.deepcopy(self.workflow_files)
                _, root = self.prepared_report()
                if mutation == 'base':
                    self.runs[0]['pull_requests'][0]['base']['sha'] = 'e' * 40
                elif mutation == 'job':
                    self.jobs[1][0]['steps'][1]['conclusion'] = 'failure'
                elif mutation == 'artifact':
                    self.producer_artifacts[0]['expired'] = True
                elif mutation == 'workflow':
                    self.workflow_files['workflow-node-1']['data']['node']['file']['repositoryFileUrl'] = None
                elif mutation == 'collector-workflow':
                    self.workflow_files['observer-901']['data']['node']['file']['repositoryFileUrl'] = (
                        f"https://github.com/owner/repo/blob/{'e' * 40}/.github/workflows/producer-custody.yaml")
                elif mutation == 'collector-branch':
                    self.observers[901]['head_branch'] = 'feature'
                elif mutation == 'collector-attempt':
                    self.observers[901]['run_attempt'] = 2
                elif mutation == 'duplicate':
                    self.runs.append(dict(self.runs[0], id=3))
                elif mutation == 'qualifier':
                    self.runs[1]['path'] += '@other'
                else:
                    self.runs[1]['name'] = 'other'
                self.assertEqual(self.sealed_report(root), 1)
                self.assertEqual(self.effects[-1][1]['state'], 'failure')
                self.assertEqual(self.effects[-1][1]['context'], CONTEXT)
                self.runs, self.jobs, self.workflow_files = runs, jobs, files
                self.config['producers']['Nix']['artifacts'] = ['nix-build-0']
                self.producer_artifacts[0]['expired'] = False
                self.folder = original

    def test_unenrolled_path_has_no_effect_even_with_invalid_identity_and_stale_receipt(self):
        payload = {'workflow_run': {'path': '.github/workflows/other.yaml', 'id': None}}
        stale = self.folder / 'receipt.json'
        stale.write_bytes(b'untouched stale receipt')
        for function in ['sealed_capture', 'sealed_aggregate', 'sealed_report']:
            with self.subTest(function=function):
                self.namespace[function](self.config, payload, self.folder)
                self.assertEqual(stale.read_bytes(), b'untouched stale receipt')
        self.assertEqual(self.calls, [])
        self.assertEqual(self.effects, [])

    def test_generated_main_captures_only_trigger_and_outputs_immutable_key(self):
        event = self.folder / 'input-event.json'
        event.write_text(json.dumps(self.payload))
        output = self.folder / 'output'
        root = self.folder / 'capture'
        with patch.dict(os.environ, {'SIMIT_CUSTODY_CONFIG': json.dumps(self.config),
                'SIMIT_CUSTODY_PHASE': 'capture', 'SIMIT_CUSTODY_ROOT': str(root),
                'GITHUB_EVENT_PATH': str(event), 'GITHUB_OUTPUT': str(output)}):
            self.assertEqual(self.namespace['sealed_main'](), 0)
        manifest = json.loads((root / 'manifest.json').read_bytes())
        self.assertEqual(output.read_text(), 'capsule_name=' + manifest['key'] + '\n')

    def test_failed_capture_uses_diagnostic_name_without_poisoning_later_capsules(self):
        event = self.folder / 'input-event.json'
        event.write_text(json.dumps(self.payload))
        original_write = self.namespace['write']
        diagnostics = []
        for index, failure in enumerate(['incomplete_logs', 'manifest_admission']):
            with self.subTest(failure=failure):
                observer_id = 902 + index
                root = self.folder / failure
                output = self.folder / (failure + '-output')
                output.write_text('')
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, 'w') as archive:
                    archive.writestr('test/1_Set up job.txt', b'nonempty pruned logs\n')
                def admit(path, value, budget=None):
                    if failure == 'manifest_admission' and path.name == 'manifest.json':
                        raise RuntimeError('manifest admission failed')
                    return original_write(path, value, budget)
                with patch.dict(self.zips, {1: stream.getvalue()} if failure == 'incomplete_logs' else {}), \
                        patch.dict(self.namespace, {'write': admit}), \
                        patch.dict(os.environ, {'SIMIT_CUSTODY_CONFIG': json.dumps(self.config),
                            'SIMIT_CUSTODY_PHASE': 'capture', 'SIMIT_CUSTODY_ROOT': str(root),
                            'GITHUB_EVENT_PATH': str(event), 'GITHUB_OUTPUT': str(output),
                            'GITHUB_RUN_ID': str(observer_id)}):
                    with self.assertRaises(RuntimeError):
                        self.namespace['sealed_main']()
                self.assertEqual(output.read_text(), '')
                self.assertEqual(json.loads((root / 'receipt.json').read_bytes())['state'], 'failed')
                self.assertFalse((root / 'manifest.json').exists())
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, 'w') as archive:
                    for path in root.rglob('*'):
                        if path.is_file():
                            archive.writestr(str(path.relative_to(root)), path.read_bytes())
                raw = stream.getvalue()
                diagnostics.append(({'id': 86 + index, 'name': f'custody-diagnostic-{observer_id}-1',
                    'expired': False, 'size_in_bytes': len(raw),
                    'digest': 'sha256:' + hashlib.sha256(raw).hexdigest(),
                    'workflow_run': {'id': observer_id, 'head_sha': 'c' * 40}}, raw))
        capsules = [self.seal_original(1, 88, 900), self.seal_original(2, 89, 901)]
        self.install_capsule_provider(diagnostics + capsules)
        for artifact, _ in diagnostics:
            self.observers[artifact['workflow_run']['id']]['conclusion'] = 'failure'
        with patch.dict(os.environ, {'SIMIT_CUSTODY_CAPSULE_ID': '88',
                'SIMIT_CUSTODY_CAPSULE_DIGEST': capsules[0][0]['digest'], 'SIMIT_CUSTODY_SEAL_OUTCOME': 'success'}):
            receipt = self.aggregate()
        self.assertEqual(receipt['state'], 'ready')
        self.assertEqual([row['capsule']['id'] for row in receipt['producers']], [88, 89])
        self.assertTrue(all(artifact['id'] in self.capsules for artifact, _ in diagnostics))

    def test_capsule_key_isolates_policy_installation_ref_event_and_full_snapshot(self):
        snapshot, _ = self.namespace['completion_identity'](self.config, self.payload)
        key = self.namespace['capsule_name']
        baseline = key(self.config, snapshot, 1, 'c' * 40)
        cases = []
        for field, value in [('branch', 'Feature'), ('event', 'push')]:
            changed = copy.deepcopy(snapshot)
            changed['target'][field] = value
            cases.append(key(self.config, changed, 1, 'c' * 40))
        changed = dict(snapshot, observed_base_sha='e' * 40)
        cases.extend([key(self.config, changed, 1, 'c' * 40), key(self.config, snapshot, 2, 'c' * 40),
                      key(self.config, snapshot, 1, 'e' * 40), key(dict(self.config, status_context='other'), snapshot, 1, 'c' * 40)])
        self.assertNotIn(baseline, cases)
        self.assertEqual(len(set(cases)), len(cases))

    def test_provider_failure_and_budget_exhaustion_still_publish_failure(self):
        for failure in ['provider', 'requests', 'expansion', 'members', 'deadline']:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                original = self.folder
                self.folder = Path(directory)
                _, root = self.prepared_report()
                if failure == 'provider':
                    original_api = self.namespace['api']
                    def unavailable(repository, endpoint, **limits):
                        if endpoint.startswith('actions/artifacts/'):
                            raise RuntimeError('provider unavailable')
                        return original_api(repository, endpoint, **limits)
                    self.namespace['api'] = unavailable
                    self.assertEqual(self.sealed_report(root), 1)
                    self.namespace['api'] = original_api
                else:
                    constant = {'requests': 'MAX_CAPTURE_REQUESTS', 'expansion': 'MAX_EXPANDED_BYTES',
                                'members': 'MAX_ARCHIVE_MEMBERS', 'deadline': 'MAX_CAPTURE_SECONDS'}[failure]
                    saved = self.namespace[constant]
                    self.namespace[constant] = 0
                    self.assertEqual(self.sealed_report(root), 1)
                    self.namespace[constant] = saved
                self.assertEqual(self.effects[-1][1]['state'], 'failure')
                self.folder = original

    def test_exhausted_post_upload_report_budget_cannot_restart_lookups(self):
        _, root = self.prepared_report()
        original_class = self.namespace['CaptureBudget']
        budgets = []
        def budget():
            value = original_class()
            budgets.append(value)
            return value
        self.namespace['CaptureBudget'] = budget
        original_api = self.namespace['api']
        aggregate_checks = 0
        def expires_at_final_artifact(repository, endpoint, **limits):
            nonlocal aggregate_checks
            raw = original_api(repository, endpoint, **limits)
            if endpoint == 'actions/artifacts/90':
                aggregate_checks += 1
                if aggregate_checks == 2:
                    budgets[-1].deadline = self.namespace['time'].monotonic() - 1
            return raw
        self.namespace['api'] = expires_at_final_artifact
        self.assertEqual(self.sealed_report(root), 1)
        self.assertEqual(self.effects[-1][1]['state'], 'failure')
        self.assertEqual(self.effects[-1][1]['context'], CONTEXT)

    def test_executed_workflow_definition_is_independent_of_raw_head(self):
        self.workflow_files['workflow-node-1']['data']['node']['file']['repositoryFileUrl'] = (
            f"https://github.com/owner/repo/blob/{'e' * 40}/.github/workflows/ci.yaml")
        original = self.namespace['api']
        def absent_at_head(repository, endpoint, **limits):
            if endpoint.startswith('contents/') and urlsplit(endpoint).query == 'ref=' + HEAD:
                raise RuntimeError('workflow absent at raw head')
            return original(repository, endpoint, **limits)
        self.namespace['api'] = absent_at_head
        self.native_provider = absent_at_head
        # Nix also uses an independently proven definition, so neither content
        # lookup has any reason to consult the producer head.
        self.workflow_files['workflow-node-2']['data']['node']['file']['repositoryFileUrl'] = (
            f"https://github.com/owner/repo/blob/{'f' * 40}/.github/workflows/nix-builds.yaml")
        receipt, root = self.prepared_report()
        self.assertEqual(receipt['head'], HEAD)
        self.assertEqual(receipt['producers'][0]['workflow_revision']['commit'], 'e' * 40)
        self.assertEqual(self.sealed_report(root), 0)

    def test_even_checksum_consistent_capsules_need_independent_semantic_admission(self):
        for mutation in ['schema', 'manifest', 'source', 'terminal', 'step']:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                original = self.folder
                self.folder = Path(directory)
                artifact, raw = self.seal_original(1, 88, 900)
                with zipfile.ZipFile(io.BytesIO(raw)) as archive:
                    files = {info.filename: archive.read(info) for info in archive.infolist()}
                receipt = json.loads(files['receipt.json'])
                if mutation == 'schema':
                    receipt['schema'] = 'simit.producer-custody.v1'
                elif mutation == 'source':
                    files['1/workflow.yaml'] = b'other source bytes'
                elif mutation == 'terminal':
                    terminal = json.loads(files['1/terminal.json'])
                    terminal['run_attempt'] = 2
                    files['1/terminal.json'] = json.dumps(terminal).encode()
                elif mutation == 'step':
                    stream = io.BytesIO()
                    with zipfile.ZipFile(io.BytesIO(files['1/provider-logs.zip'])) as before:
                        with zipfile.ZipFile(stream, 'w') as after:
                            for info in before.infolist():
                                if info.filename != 'test/2_Test.txt':
                                    after.writestr(info.filename, before.read(info))
                    files['1/provider-logs.zip'] = stream.getvalue()
                    receipt['producers'][0]['logs_sha256'] = hashlib.sha256(stream.getvalue()).hexdigest()
                    inventory = self.namespace['zip_members'](stream.getvalue())
                    files['1/provider-log-members.json'] = json.dumps(inventory).encode()
                files['receipt.json'] = json.dumps(receipt).encode()
                manifest = json.loads(files['manifest.json'])
                manifest['members'] = {name: hashlib.sha256(content).hexdigest() for name, content in files.items() if name != 'manifest.json'}
                if mutation == 'manifest':
                    manifest['members']['receipt.json'] = '0' * 64
                files['manifest.json'] = json.dumps(manifest).encode()
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, 'w') as archive:
                    for name, content in files.items():
                        archive.writestr(name, content)
                changed = stream.getvalue()
                artifact.update(size_in_bytes=len(changed), digest='sha256:' + hashlib.sha256(changed).hexdigest())
                self.install_capsule_provider([(artifact, changed)])
                snapshot, _ = self.namespace['completion_identity'](self.config, self.payload)
                with self.assertRaises(RuntimeError):
                    self.namespace['verify_capsule'](self.config, snapshot, self.runs[0], artifact,
                        'c' * 40, 'trunk', self.namespace['CaptureBudget']())
                self.folder = original

    def test_capture_seals_first_original_while_sibling_is_running(self):
        self.runs[1]['status'] = 'in_progress'
        receipt = self.namespace['sealed_capture'](self.config, self.payload, self.folder)
        self.assertTrue(receipt['capture_complete'])
        self.assertEqual([p['run_id'] for p in receipt['producers']], [1])
        self.assertEqual(receipt['schema'], 'simit.producer-capsule.v2')
        self.assertTrue((self.folder / 'manifest.json').is_file())
        self.assertFalse(any('/runs/2/' in call for call in self.calls))

    def test_nonempty_pruned_zip_cannot_be_sealed(self):
        self.zips[1] = self.zip
        with self.assertRaisesRegex(RuntimeError, 'step log'):
            self.namespace['sealed_capture'](self.config, self.payload, self.folder)
        self.assertFalse((self.folder / 'manifest.json').exists())

    def test_native_step_inventory_requires_exact_unambiguous_nonempty_members(self):
        fn = self.namespace['complete_logs']
        jobs = self.jobs[1]
        members = self.namespace['zip_members'](self.zips[1])
        self.assertEqual(len(fn(jobs, members)), 3)
        for altered in [{}, {**members, 'test/2_Test.txt': hashlib.sha256(b'').hexdigest()}]:
            with self.subTest(altered=altered), self.assertRaises(RuntimeError):
                fn(jobs, altered)
        with self.assertRaises(RuntimeError):
            fn(jobs + jobs, members)


class OptimizedSealed(Sealed):
    optimization = 2


unittest.main()
