"""Export/import only exact-source build inputs within one Actions run."""
import gzip
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def evaluate(installable, expression):
    return json.loads(subprocess.check_output(['nix', 'eval', '--json', '--no-update-lock-file',
                                               installable, '--apply', expression], text=True))


root = Path(os.environ['SIMIT_DEPENDENCY_ROOT'])
root.mkdir(exist_ok=True)
mode = os.environ['SIMIT_DEPENDENCY_MODE']
maximum = int(os.environ['SIMIT_DEPENDENCY_MAX_BYTES'])
dependencies = json.loads(os.environ['SIMIT_NIX_DEPENDENCIES'])
plan = json.loads(os.environ['SIMIT_NIX_PLAN'])
systems = json.loads(os.environ['SIMIT_NIX_CHECK_SYSTEMS'])
limits = json.loads(os.environ['SIMIT_NIX_LIMITS'])
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], text=True).strip()
expected_head = os.environ['GITHUB_SHA']
if os.environ['GITHUB_EVENT_NAME'] == 'pull_request':
    expected_head = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['pull_request']['head']['sha']
assert head == expected_head, 'Dependency source does not match the event head'
subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--'], check=True)
identity = {'schema': 'simit.nix-dependencies.v1', 'head': head, 'tree': tree,
            'repository': os.environ['GITHUB_REPOSITORY'], 'run': os.environ['GITHUB_RUN_ID'],
            'attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
            'flake_lock_sha256': digest(Path('flake.lock')) if Path('flake.lock').is_file() else None}
assert dependencies and systems and len(set(dependencies)) == len(dependencies)
assert not set(dependencies) & set(plan)
check_drvs = set()
for system in systems:
    checks = evaluate(f'.#checks.{system}', 'checks: builtins.mapAttrs (_: d: d.drvPath) checks')
    assert isinstance(checks, dict) and checks, 'Invalid discovered check inventory'
    check_drvs.update(checks.values())
expression = 'd: { drvPath = d.drvPath; outputs = builtins.listToAttrs (map (name: { inherit name; value = d.${name}.outPath; }) d.outputs); }'
definitions = {installable: evaluate(installable, expression) for installable in dependencies}
qualified_drvs = {evaluate(installable, 'd: d.drvPath') for installable in plan}
for definition in definitions.values():
    assert definition['drvPath'] not in check_drvs | qualified_drvs, 'A qualification derivation was enrolled as a build input'
    assert definition['outputs'] and all(re.fullmatch(r'/nix/store/[a-z0-9]{32}-[^/\s]+', path)
                                         for path in definition['outputs'].values())
manifest_path = root / 'manifest.json'
archive = root / 'closure.nar.gz'
if mode == 'export':
    manifest = {**identity, 'definitions': definitions, 'complete': False}
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
    outputs = set()
    for index, (installable, definition) in enumerate(definitions.items()):
        with (root / f'build-{index}.log').open('wb') as log:
            result = subprocess.run(['nix', 'build', '--no-link', '--no-update-lock-file',
                                     '--print-build-logs', '--json', *limits, installable + '^*'],
                                    stdout=subprocess.PIPE, stderr=log)
        (root / f'result-{index}.json').write_bytes(result.stdout)
        result.check_returncode()
        realized = json.loads(result.stdout)
        assert len(realized) == 1 and realized[0]['drvPath'] == definition['drvPath']
        assert realized[0]['outputs'] == definition['outputs'], 'Dependency outputs differ from their evaluated identity'
        outputs.update(definition['outputs'].values())
    closure_data = json.loads(subprocess.check_output(['nix', 'path-info', '--json', '--recursive', *sorted(outputs)], text=True))
    if isinstance(closure_data, list):
        closure_data = {row['path']: row for row in closure_data}
    assert isinstance(closure_data, dict) and closure_data
    assert sum(row['narSize'] for row in closure_data.values()) <= maximum, 'Dependency closure exceeds its transport budget'
    closure = sorted(closure_data)
    assert all(re.fullmatch(r'/nix/store/[a-z0-9]{32}-[^/\s]+', path) for path in closure)
    with (root / 'export.log').open('wb') as log, gzip.open(archive, 'wb', compresslevel=1) as compressed:
        with subprocess.Popen(['nix-store', '--export', *closure], stdout=subprocess.PIPE, stderr=log) as producer:
            size = 0
            while chunk := producer.stdout.read(1024 * 1024):
                size += len(chunk)
                if size > maximum:
                    producer.terminate()
                    producer.wait(timeout=30)
                    raise RuntimeError('Export stream exceeds transport budget')
                compressed.write(chunk)
            assert producer.wait() == 0, 'Dependency export failed'
    manifest.update(complete=True, closure=closure, uncompressed_bytes=size,
                    archive_sha256=digest(archive), archive_bytes=archive.stat().st_size)
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
elif mode == 'import':
    assert manifest_path.is_file() and not manifest_path.is_symlink()
    manifest = json.loads(manifest_path.read_text())
    assert all(manifest.get(key) == value for key, value in identity.items()), 'Dependency provenance differs from this source/run'
    assert manifest['complete'] is True and manifest['definitions'] == definitions
    assert archive.is_file() and not archive.is_symlink() and archive.stat().st_size == manifest['archive_bytes']
    assert archive.stat().st_size <= maximum + 1024 * 1024
    assert digest(archive) == manifest['archive_sha256'], 'Dependency archive checksum mismatch'
    plain = root / 'import.nar'
    with gzip.open(archive, 'rb') as compressed, plain.open('xb') as stream:
        size = 0
        while chunk := compressed.read(1024 * 1024):
            size += len(chunk)
            assert size <= maximum, 'Decompressed dependency transport exceeds budget'
            stream.write(chunk)
    assert size == manifest['uncompressed_bytes'], 'Dependency transport was truncated'
    with plain.open('rb') as stream, (root / 'import.log').open('wb') as log:
        subprocess.run(['nix-store', '--import'], stdin=stream, stdout=log, stderr=subprocess.STDOUT, check=True)
    subprocess.run(['nix-store', '--verify-path', *manifest['closure']], check=True)
    paths = sorted(path for definition in definitions.values() for path in definition['outputs'].values())
    subprocess.run(['nix', 'path-info', *paths], check=True)
    plain.unlink()
    (root / 'import-receipt.json').write_text(json.dumps({**identity, 'archive_sha256': manifest['archive_sha256'],
                                                       'restored': paths, 'qualification_execution': False}, indent=2) + '\n')
else:
    raise RuntimeError('Unknown dependency transport operation')
