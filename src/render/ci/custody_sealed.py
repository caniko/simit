"""Prospective v2: complete per-original evidence, immutable capsules, native revocation.

Appended to the legacy program by the renderer. Legacy capture/report remain
available, but neither legacy receipts nor fresh ephemeral ZIPs fill v2 custody.
"""

CAPSULE_SCHEMA = 'simit.producer-capsule.v2'
AGGREGATE_SCHEMA = 'simit.producer-custody.v2'
OBSERVER_PATH = '.github/workflows/producer-custody.yaml'


def canonical_hash(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def capsule_name(config, snapshot, run_id, installation):
    return 'producer-capsule-v2-' + canonical_hash({
        'policy': config, 'snapshot': snapshot, 'run_id': run_id,
        'attempt': 1, 'installation': installation})


def complete_logs(jobs, members):
    """Require every executed native step, including setup and post-job cleanup.

    v2 deliberately admits only exact provider paths. Unsupported filename
    transformations fail closed; a whole-job rollup is never step completeness.
    Skipped jobs/steps need no log. Duplicate names/numbers cannot map uniquely.
    """
    require(jobs and len(jobs) <= 100, 'Native job inventory exceeds capsule admission')
    names, ids, expected = set(), set(), []
    for job in jobs:
        name = job['name']
        require(isinstance(name, str) and name and '/' not in name and '\\' not in name
                and name not in ('.', '..') and name not in names
                and type(job['id']) is int and job['id'] > 0 and job['id'] not in ids,
                'Ambiguous native job log identity')
        names.add(name)
        ids.add(job['id'])
        require(job['status'] == 'completed', 'Incomplete native job')
        if job['conclusion'] == 'skipped':
            continue
        steps = job['steps']
        require(isinstance(steps, list) and steps, 'Missing native step inventory')
        numbers = set()
        for step in steps:
            number, step_name = step['number'], step['name']
            require(type(number) is int and number > 0 and number not in numbers
                    and isinstance(step_name, str) and step_name and '/' not in step_name
                    and '\\' not in step_name, 'Ambiguous native step log identity')
            numbers.add(number)
            require(step['status'] == 'completed', 'Incomplete native step')
            if step['conclusion'] == 'skipped':
                continue
            path = f'{name}/{number}_{step_name}.txt'
            require(path in members and members[path] != hashlib.sha256(b'').hexdigest(),
                    'Missing or empty native step log: ' + path)
            expected.append(path)
    require(expected and len(expected) == len(set(expected)), 'Missing or duplicate native step logs')
    return expected


def file_hashes(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob('*')) if path.is_file() and path.name != 'manifest.json'}


def sealed_capture(config, payload, root, budget=None):
    if unenrolled_completion(config, payload):
        return None
    budget = budget or CaptureBudget()
    snapshot, _ = completion_identity(config, payload)
    event_run = payload['workflow_run']
    require(os.environ['GITHUB_RUN_ATTEMPT'] == '1', 'Capsules require an original observer attempt')
    branch_identity(payload['repository']['default_branch'])
    write(root / 'event.json', payload, budget)
    require(event_run['name'] in config['producers'], 'Trigger workflow is not enrolled')
    single = dict(config, producers={event_run['name']: config['producers'][event_run['name']]})
    # No sibling can delay downloading this completed original's ephemeral logs.
    receipt = capture(single, payload, root, budget)
    require(receipt['capture_complete'] and len(receipt['producers']) == 1,
            'Original capsule capture incomplete')
    row = receipt['producers'][0]
    folder = root / str(row['run_id'])
    jobs = json.loads((folder / 'jobs.json').read_bytes())
    members = json.loads((folder / 'provider-log-members.json').read_bytes())
    row['complete_step_logs'] = complete_logs(jobs, members)
    # Fence native jobs and artifact inventory independently of the source snapshot.
    repository = repository_identity(config['repository'])
    require(pages(repository, f"actions/runs/{row['run_id']}/attempts/1/jobs", 'jobs', budget.request) == jobs,
            'Native job inventory changed during sealing')
    artifacts = json.loads((folder / 'artifacts.json').read_bytes())
    require(pages(repository, f"actions/runs/{row['run_id']}/artifacts", 'artifacts', budget.request) == artifacts,
            'Native artifact inventory changed during sealing')
    for retained in row['artifacts']:
        artifact = next(value for value in artifacts if value['id'] == retained['id'])
        require(artifact.get('digest') == 'sha256:' + retained['sha256'], 'Missing immutable producer artifact digest')
    raw_commit = budget.request(repository, 'git/commits/' + receipt['head'])
    require(json.loads(raw_commit)['tree']['sha'] == receipt['tree'], 'Source tree changed during sealing')
    budget.retain(root / 'source-commit.json', raw_commit)
    receipt.update(schema=CAPSULE_SCHEMA, policy=config, snapshot=snapshot,
                   default_branch=payload['repository']['default_branch'])
    write(root / 'receipt.json', receipt, budget)
    require(all(path.stat().st_size <= MAX_EVENT_BYTES for path in root.rglob('*.json')),
            'Sealed JSON exceeds admission bound')
    manifest = {'schema': CAPSULE_SCHEMA, 'key': capsule_name(config, snapshot, row['run_id'], receipt['collector_head']),
                'members': file_hashes(root)}
    write(root / 'manifest.json', manifest, budget)
    return receipt


def discover_originals(config, snapshot, fetch):
    repository = repository_identity(config['repository'])
    candidates = pages(repository, 'actions/runs?' + urlencode({'head_sha': snapshot['head'],
                       'event': snapshot['target']['event']}), 'workflow_runs', fetch)
    selected = {}
    for name, producer in config['producers'].items():
        matches = []
        for run in candidates:
            if (not isinstance(run.get('path'), str) or run['path'].partition('@')[0] != producer['workflow']
                    or run['head_sha'] != snapshot['head'] or run['head_branch'] != snapshot['target']['branch']
                    or run['event'] != snapshot['target']['event']):
                continue
            identity = run_identity(run, repository)
            if identity['target'] != snapshot['target']:
                continue
            require(identity == snapshot, 'Sibling observed snapshot differs')
            validate_run(run, snapshot['head'], snapshot['target']['branch'], snapshot['target']['event'],
                         name, producer, 1, repository, snapshot)
            matches.append(run)
        require(len(matches) <= 1, 'Duplicate original producer identity')
        if matches:
            run = json.loads(fetch(repository, f"actions/runs/{matches[0]['id']}"))
            validate_run(run, snapshot['head'], snapshot['target']['branch'], snapshot['target']['event'],
                         name, producer, 1, repository, snapshot)
            selected[name] = run
    return selected


def digest_value(value):
    require(isinstance(value, str), 'Missing immutable artifact digest')
    value = value.removeprefix('sha256:')
    require(re.fullmatch(r'[0-9a-f]{64}', value), 'Invalid immutable artifact digest')
    return 'sha256:' + value


def artifact_identity(artifact):
    require(type(artifact['id']) is int and artifact['id'] > 0 and artifact['expired'] is False
            and type(artifact['size_in_bytes']) is int and 0 < artifact['size_in_bytes'] <= MAX_BYTES,
            'Invalid, expired or oversized sealed artifact')
    digest_value(artifact['digest'])
    return {key: artifact[key] for key in ['id', 'name', 'size_in_bytes', 'digest', 'workflow_run']}


def observer_identity(config, artifact, installation, default_branch, fetch):
    repository = repository_identity(config['repository'])
    owner = artifact['workflow_run']
    require(type(owner['id']) is int and owner['id'] > 0 and owner['head_sha'] == installation,
            'Capsule collector installation differs')
    run = json.loads(fetch(repository, f"actions/runs/{owner['id']}"))
    require(type(run['id']) is int and run['id'] == owner['id']
            and type(run['run_attempt']) is int and run['run_attempt'] == 1
            and run['event'] == 'workflow_run' and run['head_sha'] == installation
            and run['head_branch'] == default_branch and run['path'] == OBSERVER_PATH
            and run['name'] == 'Retain completed producer custody'
            and repository_identity(run['repository']['full_name']) == repository
            == repository_identity(run['head_repository']['full_name'])
            and run['repository']['id'] == run['head_repository']['id'], 'Untrusted capsule collector run')
    if str(run['id']) != os.environ['GITHUB_RUN_ID']:
        require(run['status'] == 'completed', 'Prior capsule collector is not terminal')
    else:
        require(run['status'] == 'in_progress' or (run['status'] == 'completed' and run['conclusion'] == 'success'),
                'Current capsule collector is not eligible')
    revision = workflow_revision(run, OBSERVER_PATH, repository, fetch)
    require(revision['commit'] == installation, 'Capsule collector executed another workflow revision')
    jobs = pages(repository, f"actions/runs/{run['id']}/attempts/1/jobs", 'jobs', fetch)
    seals = [job for job in jobs if job['name'] == 'seal']
    # Actions can replace a pending aggregate even with cancel-in-progress:false.
    # A successfully sealed capsule survives that cancellation; the native seal
    # job, not the sibling aggregate's result, proves the capture execution.
    require(len(seals) == 1 and seals[0]['status'] == 'completed' and seals[0]['conclusion'] == 'success'
            and type(seals[0]['id']) is int and seals[0]['id'] > 0
            and seals[0]['run_id'] == run['id'] and seals[0]['run_attempt'] == 1
            and seals[0]['head_sha'] == installation, 'Capsule collector seal job did not succeed')
    return run


def sealed_archive(config, artifact, installation, default_branch, budget):
    repository = repository_identity(config['repository'])
    identity = artifact_identity(artifact)
    expected_digest = digest_value(identity['digest'])
    observer = observer_identity(config, artifact, installation, default_branch, budget.request)
    raw = budget.request(repository, f"actions/artifacts/{identity['id']}/zip")
    require(len(raw) == identity['size_in_bytes']
            and expected_digest == 'sha256:' + hashlib.sha256(raw).hexdigest(), 'Sealed artifact digest or size differs')
    members = zip_members(raw, budget)
    return raw, members, observer


def archive_json(archive, path):
    info = archive.getinfo(path)
    require(info.file_size <= MAX_EVENT_BYTES, 'Sealed JSON exceeds admission bound')
    return json.loads(archive.read(path))


def verify_capsule(config, snapshot, run, artifact, installation, default_branch, budget):
    raw, members, observer = sealed_archive(config, artifact, installation, default_branch, budget)
    repository = repository_identity(config['repository'])
    name = run['name']
    producer = config['producers'][name]
    expected_key = capsule_name(config, snapshot, run['id'], installation)
    require(artifact['name'] == expected_key, 'Capsule key differs')
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        manifest = archive_json(archive, 'manifest.json')
        require(manifest['schema'] == CAPSULE_SCHEMA and manifest['key'] == expected_key
                and manifest['members'] == {path: checksum for path, checksum in members.items() if path != 'manifest.json'},
                'Capsule member manifest differs')
        receipt = archive_json(archive, 'receipt.json')
        require(receipt['schema'] == CAPSULE_SCHEMA and receipt['policy'] == config and receipt['snapshot'] == snapshot
                and receipt['capture_complete'] is True and receipt['qualified'] is False
                and receipt['repository'] == repository and receipt['head'] == snapshot['head']
                and receipt['branch'] == snapshot['target']['branch'] and receipt['event'] == snapshot['target']['event']
                and receipt['collector_head'] == installation and receipt['collector_run'] == str(observer['id'])
                and type(receipt['collector_attempt']) is int and receipt['collector_attempt'] == 1
                and receipt['default_branch'] == default_branch and len(receipt['producers']) == 1,
                'Capsule policy, source, collector or snapshot differs')
        event = archive_json(archive, 'event.json')
        require(run_identity(event['workflow_run'], repository) == snapshot
                and event['workflow_run']['id'] == run['id']
                and event['repository']['id'] == snapshot['target']['repository']['id'], 'Capsule trigger differs')
        commit = archive_json(archive, 'source-commit.json')
        require(commit['sha'] == snapshot['head'] and re.fullmatch(r'[0-9a-f]{40}', commit['tree']['sha'])
                and commit['tree']['sha'] == receipt['tree'], 'Capsule source tree differs')
        row = receipt['producers'][0]
        require(row['run_id'] == run['id'] and row['name'] == name and row['workflow'] == producer['workflow']
                and row['attempt'] == 1 and row['snapshot'] == snapshot and row['event'] == run['event']
                and row['conclusion'] == run['conclusion'] and run['status'] == 'completed', 'Capsule original differs')
        folder = str(run['id']) + '/'
        terminal = archive_json(archive, folder + 'terminal.json')
        validate_run(terminal, snapshot['head'], snapshot['target']['branch'], snapshot['target']['event'],
                     name, producer, 1, repository, snapshot)
        require(terminal['status'] == 'completed' and terminal['conclusion'] == run['conclusion'], 'Capsule terminal differs')
        proof = archive.read(folder + 'workflow-run-file.json')
        revision = workflow_revision(terminal, producer['workflow'], repository, lambda *args, **kwargs: proof)
        require(revision == row['workflow_revision']
                == workflow_revision(run, producer['workflow'], repository, budget.request), 'Capsule native executed file differs')
        workflow_commit = archive_json(archive, folder + 'workflow-commit.json')
        source = archive_json(archive, folder + 'workflow-content.json')
        workflow = archive.read(folder + 'workflow.yaml')
        require(workflow_commit['sha'] == revision['commit'] and workflow_commit['tree']['sha'] == row['workflow_tree']
                and source['type'] == 'file' and source['encoding'] == 'base64'
                and base64.b64decode(source['content']) == workflow
                and hashlib.sha1(b'blob ' + str(len(workflow)).encode() + b'\0' + workflow).hexdigest() == source['sha']
                and members[folder + 'workflow.yaml'] == row['source_workflow_sha256']
                and receipt['source_workflow_sha256'] == {producer['workflow']: row['source_workflow_sha256']},
                'Capsule workflow commit, blob or bytes differ')
        fresh_commit = json.loads(budget.request(repository, 'git/commits/' + revision['commit']))
        fresh_source = json.loads(budget.request(repository, 'contents/' + quote(producer['workflow'], safe='/')
                                               + '?ref=' + revision['commit']))
        require(fresh_commit['sha'] == revision['commit'] and fresh_commit['tree']['sha'] == row['workflow_tree']
                and fresh_source['type'] == 'file' and fresh_source['encoding'] == 'base64'
                and fresh_source['sha'] == source['sha'] and base64.b64decode(fresh_source['content']) == workflow,
                'Native immutable workflow source differs from capsule')
        jobs = archive_json(archive, folder + 'jobs.json')
        require(all(job['run_id'] == run['id'] and job['head_sha'] == snapshot['head']
                    and job['run_attempt'] == 1 for job in jobs), 'Capsule job source or attempt differs')
        log_zip = archive.read(folder + 'provider-logs.zip')
        require('sha256:' + row['logs_sha256'] == 'sha256:' + hashlib.sha256(log_zip).hexdigest(), 'Capsule provider ZIP checksum differs')
        log_members = zip_members(log_zip, budget)
        require(log_members == archive_json(archive, folder + 'provider-log-members.json')
                and complete_logs(jobs, log_members) == row['complete_step_logs'], 'Capsule log completeness differs')
        required = {job_name: [job for job in jobs if job['name'] == job_name] for job_name in producer['jobs']}
        require(all(len(rows) == 1 for rows in required.values()), 'Missing or duplicate enrolled jobs')
        require(row['jobs_accepted'] == all(rows[0]['conclusion'] == 'success' for rows in required.values())
                and len(row['job_logs']) == len(required), 'Capsule required job acceptance differs')
        for job_name, rows in required.items():
            job = rows[0]
            logs = [value for value in row['job_logs'] if value['id'] == job['id'] and value['name'] == job_name]
            require(len(logs) == 1 and members[folder + f"job-{job['id']}.log"] == logs[0]['sha256']
                    and logs[0]['sha256'] != hashlib.sha256(b'').hexdigest(), 'Capsule enrolled job log differs')
        artifacts = archive_json(archive, folder + 'artifacts.json')
        require(len(row['artifacts']) == len(producer['artifacts']), 'Capsule enrolled artifact inventory differs')
        for artifact_name in producer['artifacts']:
            originals = [value for value in artifacts if value['name'] == artifact_name]
            retained = [value for value in row['artifacts'] if value['name'] == artifact_name]
            require(len(originals) == len(retained) == 1, 'Missing or duplicate enrolled artifacts')
            original, retained = originals[0], retained[0]
            require(original['id'] == retained['id'] and original['expired'] is False
                    and original['workflow_run']['id'] == run['id'] and original['workflow_run']['head_sha'] == snapshot['head'],
                    'Capsule enrolled artifact source differs')
            zipped = archive.read(folder + f"artifact-{original['id']}.zip")
            require(original['digest'] == 'sha256:' + retained['sha256']
                    == 'sha256:' + hashlib.sha256(zipped).hexdigest()
                    and zip_members(zipped, budget) == retained['members'], 'Capsule enrolled artifact bytes differ')
        require(pages(repository, f"actions/runs/{run['id']}/attempts/1/jobs", 'jobs', budget.request) == jobs,
                'Native job inventory changed after sealing')
        require(pages(repository, f"actions/runs/{run['id']}/artifacts", 'artifacts', budget.request) == artifacts,
                'Native artifact inventory changed after sealing')
    # Recheck immutable identity after download; deletion/expiry is not reuse.
    fresh = json.loads(budget.request(repository, f"actions/artifacts/{artifact['id']}"))
    require(artifact_identity(fresh) == artifact_identity(artifact), 'Sealed artifact identity changed during verification')
    require(observer['repository']['id'] == snapshot['target']['repository']['id'], 'Capsule collector repository ID differs')
    return dict(row, capsule=artifact_identity(artifact), tree=receipt['tree'],
                native_jobs_sha256=canonical_hash(jobs), native_artifacts_sha256=canonical_hash(artifacts))


def aggregate_receipt(config, payload):
    snapshot, context = completion_identity(config, payload)
    return {'schema': AGGREGATE_SCHEMA, 'repository': repository_identity(config['repository']),
            'head': snapshot['head'], 'branch': snapshot['target']['branch'], 'event': snapshot['target']['event'],
            'snapshot': snapshot, 'status_context': context, 'policy': config, 'producers': [],
            'collector_head': os.environ['GITHUB_SHA'], 'collector_run': os.environ['GITHUB_RUN_ID'],
            'collector_attempt': int(os.environ['GITHUB_RUN_ATTEMPT']), 'default_branch': payload['repository']['default_branch'],
            'state': 'waiting', 'capture_complete': False, 'qualified': False, 'observation_is_execution': False}


def sealed_aggregate(config, payload, root, budget=None):
    if unenrolled_completion(config, payload):
        return None
    receipt = aggregate_receipt(config, payload)
    write(root / 'receipt.json', receipt)
    budget = budget or CaptureBudget()
    require(os.environ['GITHUB_RUN_ATTEMPT'] == '1', 'Aggregation requires an original observer attempt')
    require(os.environ['SIMIT_CUSTODY_SEAL_OUTCOME'] == 'success', 'Original capsule upload failed')
    current_id = os.environ['SIMIT_CUSTODY_CAPSULE_ID']
    require(current_id.isdigit() and int(current_id) > 0, 'Missing original capsule upload identity')
    current_digest = digest_value(os.environ['SIMIT_CUSTODY_CAPSULE_DIGEST'])
    repository, snapshot = receipt['repository'], receipt['snapshot']
    selected = discover_originals(config, snapshot, budget.request)
    trigger = payload['workflow_run']
    require(trigger['name'] in selected and selected[trigger['name']]['id'] == trigger['id']
            and selected[trigger['name']]['status'] == 'completed', 'Completion original missing from discovery')
    missing = len(selected) != len(config['producers'])
    for name, run in selected.items():
        if run['status'] != 'completed':
            missing = True
            continue
        key = capsule_name(config, snapshot, run['id'], receipt['collector_head'])
        artifacts = pages(repository, 'actions/artifacts?' + urlencode({'name': key}), 'artifacts', budget.request)
        require(all(artifact['name'] == key for artifact in artifacts) and len(artifacts) <= 1, 'Duplicate or aliased original capsules')
        if not artifacts:
            require(run['id'] != trigger['id'], 'Uploaded original capsule missing from provider inventory')
            missing = True
            continue
        artifact = artifacts[0]
        if run['id'] == trigger['id']:
            require(artifact['id'] == int(current_id) and digest_value(artifact['digest']) == current_digest,
                    'Original capsule upload ID or digest differs')
        row = verify_capsule(config, snapshot, run, artifact, receipt['collector_head'], receipt['default_branch'], budget)
        receipt['producers'].append(row)
    failed = any(run['status'] == 'completed' and run['conclusion'] != 'success' for run in selected.values())
    failed = failed or any(not row['jobs_accepted'] for row in receipt['producers'])
    receipt['capture_complete'] = not missing
    receipt['state'] = 'failed' if failed else 'waiting' if missing else 'ready'
    receipt['capture_budget'] = {'downloaded_bytes': budget.downloaded, 'expanded_bytes': budget.expanded,
                                 'requests': budget.requests, 'archive_members': budget.members}
    write(root / 'receipt.json', receipt, budget)
    require((root / 'receipt.json').stat().st_size <= MAX_EVENT_BYTES, 'Sealed JSON exceeds admission bound')
    return receipt


def sealed_report(config, payload, root):
    if unenrolled_completion(config, payload):
        return 0
    expected = aggregate_receipt(config, payload)
    receipt = expected
    budget = CaptureBudget()
    repository = expected['repository']
    try:
        receipt = json.loads((root / 'receipt.json').read_bytes())
        for key in ['schema', 'repository', 'head', 'branch', 'event', 'snapshot', 'status_context',
                    'policy', 'collector_head', 'collector_run', 'collector_attempt', 'default_branch']:
            require(receipt[key] == expected[key], 'Aggregate reporting identity differs: ' + key)
        require(receipt['state'] in ['ready', 'waiting'], 'Aggregate capture failed')
        require(os.environ['SIMIT_CUSTODY_UPLOAD_OUTCOME'] == 'success', 'Aggregate upload failed')
        artifact_id = os.environ['SIMIT_CUSTODY_ARTIFACT_ID']
        require(artifact_id.isdigit() and int(artifact_id) > 0, 'Missing aggregate upload identity')
        artifact = json.loads(budget.request(repository, f'actions/artifacts/{int(artifact_id)}'))
        require(artifact['name'] == f"producer-custody-v2-{expected['collector_run']}-1"
                and artifact['workflow_run']['id'] == int(expected['collector_run'])
                and digest_value(artifact['digest']) == digest_value(os.environ['SIMIT_CUSTODY_ARTIFACT_DIGEST']),
                'Aggregate upload ID, owner or digest differs')
        raw, members, observer = sealed_archive(config, artifact, expected['collector_head'], expected['default_branch'], budget)
        require(observer['repository']['id'] == expected['snapshot']['target']['repository']['id'],
                'Aggregate collector repository ID differs')
        require(members == file_hashes(root), 'Uploaded aggregate bytes differ from reporting receipt')
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            require(archive_json(archive, 'receipt.json') == receipt, 'Uploaded aggregate receipt differs')
        selected = discover_originals(config, expected['snapshot'], budget.request)
        require(all(run['status'] != 'completed' or run['conclusion'] == 'success' for run in selected.values()),
                'Native original failed before reporting')
        rows = receipt['producers']
        require(len(rows) == len({row['name'] for row in rows}), 'Duplicate aggregate producer inventory')
        if receipt['state'] == 'ready':
            require(receipt['capture_complete'] is True and set(selected) == set(config['producers'])
                    == {row['name'] for row in rows}, 'Aggregate original inventory incomplete')
        for row in rows:
            require(row['name'] in selected and selected[row['name']]['id'] == row['run_id'], 'Aggregate original disappeared')
            key = capsule_name(config, expected['snapshot'], row['run_id'], expected['collector_head'])
            capsules = pages(repository, 'actions/artifacts?' + urlencode({'name': key}), 'artifacts', budget.request)
            require(len(capsules) == 1 and artifact_identity(capsules[0]) == row['capsule'],
                    'Sealed capsule deleted, replaced, expired or duplicated')
            verified = verify_capsule(config, expected['snapshot'], selected[row['name']], capsules[0],
                                      expected['collector_head'], expected['default_branch'], budget)
            require(verified == row, 'Aggregate capsule binding differs')
        if receipt['state'] == 'ready':
            require(all(row['conclusion'] == 'success' and row['jobs_accepted'] is True for row in rows),
                    'Aggregate original or required job failed')
        # Final native checks after all sealed bytes have been read. No producer
        # ZIP endpoint participates: the uploaded ID/digest is the durable fence.
        final = discover_originals(config, expected['snapshot'], budget.request)
        require({name: (run['id'], run['status'], run['conclusion']) for name, run in final.items()}
                == {name: (run['id'], run['status'], run['conclusion']) for name, run in selected.items()},
                'Original inventory changed before reporting')
        for row in rows:
            run_id = row['run_id']
            require(workflow_revision(final[row['name']], row['workflow'], repository, budget.request)
                    == row['workflow_revision'], 'Native executed workflow changed before reporting')
            require(canonical_hash(pages(repository, f'actions/runs/{run_id}/attempts/1/jobs', 'jobs', budget.request))
                    == row['native_jobs_sha256'], 'Native jobs changed before reporting')
            require(canonical_hash(pages(repository, f'actions/runs/{run_id}/artifacts', 'artifacts', budget.request))
                    == row['native_artifacts_sha256'], 'Native artifacts changed before reporting')
            fresh = json.loads(budget.request(repository, f"actions/artifacts/{row['capsule']['id']}"))
            require(artifact_identity(fresh) == row['capsule'], 'Sealed capsule changed before reporting')
            observer_identity(config, fresh, expected['collector_head'], expected['default_branch'], budget.request)
        fresh = json.loads(budget.request(repository, f"actions/artifacts/{artifact['id']}"))
        require(artifact_identity(fresh) == artifact_identity(artifact), 'Aggregate artifact changed before reporting')
        if receipt['state'] == 'waiting':
            # A missing capsule can be pending during the first completion, but
            # can never downgrade an earlier accepted set into harmless waiting.
            previous = pages(repository, f"commits/{receipt['head']}/status", 'statuses', budget.request)
            current = next((status for status in previous if status['context'] == receipt['status_context']), None)
            require(current is None or current['state'] != 'success', 'Previously accepted capsule set is now incomplete')
    except Exception as error:
        # Establish the target from this event, never a stale/malformed receipt.
        receipt = dict(expected, state='failed', report_error=str(error))
        write(root / 'receipt.json', receipt)
        write(root / 'report-failure.json', receipt)
    return publish_sealed_status(config, root, receipt, budget)


def publish_sealed_status(config, root, receipt, budget):
    """Reuse the report's lookup budget and reserve an independent failure POST.

    Starting another legacy six-minute report budget here would crowd the
    25-minute job's publication reserve after aggregation and artifact upload.
    """
    repository = repository_identity(config['repository'])
    context = status_context(config, receipt['snapshot']['target'])
    require(receipt['status_context'] == context, 'Reporting context differs')
    state = 'success' if receipt['state'] == 'ready' else 'pending' if receipt['state'] == 'waiting' else 'failure'
    identity = hashlib.sha256(json.dumps({'policy': config, 'producers': receipt['producers']}, sort_keys=True).encode()).hexdigest()[:16]
    try:
        previous = pages(repository, f"commits/{receipt['head']}/status", 'statuses', budget.request)
    except Exception as error:
        receipt.update(state='failed', status_lookup_error=str(error))
        write(root / 'report-failure.json', receipt)
        state, previous = 'failure', []
    description = f'Original-producer custody {state}: {identity}'
    current = next((status for status in previous if status['context'] == context), None)
    if current is None or current['state'] != state or current['description'] != description:
        data = {'state': state, 'context': context, 'description': description,
                'target_url': f"https://github.com/{repository}/actions/runs/{os.environ['GITHUB_RUN_ID']}"}
        subprocess.run(['gh', 'api', '--hostname', 'github.com', '--method', 'POST',
                        f"repos/{repository}/statuses/{receipt['head']}", '--input', '-'],
                       input=json.dumps(data).encode(), check=True, timeout=90)
    return 1 if state == 'failure' else 0


def sealed_main():
    config = json.loads(os.environ['SIMIT_CUSTODY_CONFIG'])
    phase = os.environ['SIMIT_CUSTODY_PHASE']
    with Path(os.environ['GITHUB_EVENT_PATH']).open('rb') as event:
        raw = event.read(MAX_EVENT_BYTES + 1)
    require(len(raw) <= MAX_EVENT_BYTES, 'Completion event exceeds its bound')
    payload = json.loads(raw)
    # Repeat the same early no-effect fence in every phase, including report.
    if unenrolled_completion(config, payload):
        if phase == 'identity':
            with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
                output.write('eligible=false\n')
        return 0
    if phase == 'identity':
        return main()
    root = Path(os.environ['SIMIT_CUSTODY_ROOT'])
    root.mkdir(exist_ok=True)
    if phase == 'report':
        return sealed_report(config, payload, root)
    require(phase in ['capture', 'aggregate'], 'Unknown sealed custody phase')
    expected = aggregate_receipt(config, payload)
    write(root / 'receipt.json', expected)
    try:
        if phase == 'capture':
            sealed_capture(config, payload, root)
            snapshot = expected['snapshot']
            key = capsule_name(config, snapshot, payload['workflow_run']['id'], expected['collector_head'])
            with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
                output.write('capsule_name=' + key + '\n')
        else:
            sealed_aggregate(config, payload, root)
    except Exception as error:
        write(root / 'receipt.json', dict(expected, state='failed', error=str(error)))
        raise
    return 0


if __name__ == '__main__':
    raise SystemExit(sealed_main())
