"""Execute the exact executor assets on fresh hosted workers with real Nix."""
import json
import os
import subprocess
import sys
from pathlib import Path

source = Path(__file__).resolve().parent.parent
program = (source / 'src/render/ci/nix_dependencies.py').read_text()
group = (source / 'src/render/ci/nix_group.py').read_text()
os.chdir(source / 'tests/fixtures/nix-transport')
assert os.environ['RUNNER_ENVIRONMENT'] == 'github-hosted'
operation = sys.argv[1]
root = Path(os.environ['RUNNER_TEMP']) / 'transport-qualification'
root.mkdir(exist_ok=True)
members = ['.#checks.x86_64-linux.runtime', '.#checks.x86_64-linux.sibling']
os.environ.update(SIMIT_DEPENDENCY_ROOT=str(Path(os.environ['RUNNER_TEMP']) / 'simit-nix-dependencies'),
                  SIMIT_DEPENDENCY_MAX_BYTES=str(64 * 1024 * 1024), SIMIT_NIX_DEPENDENCIES='[".#input"]',
                  SIMIT_NIX_PLAN=json.dumps(members), SIMIT_NIX_CHECK_SYSTEMS='["x86_64-linux"]',
                  SIMIT_NIX_LIMITS='["--max-jobs","1","--cores","2"]',
                  SIMIT_DEPENDENCY_MODE='export' if operation == 'export' else 'import')
output = subprocess.check_output(['nix', 'eval', '--raw', '--no-update-lock-file', '.#input.outPath'], text=True)
before = Path(output).exists()
(root / 'dependency-before-import.json').write_text(json.dumps({'path': output, 'present': before}) + '\n')
if operation != 'export':
    assert not before, 'Consumer is not a fresh worker with absent transported input'
exec(compile(program, '<exact-dependency-executor>', 'exec'), {'__name__': '__main__'})
if operation == 'export':
    raise SystemExit(0)
assert Path(output).is_file(), 'Exact dependency output was not restored'
if operation == 'warm':
    with (root / 'warm-materialization.log').open('wb') as log:
        subprocess.run(['nix', 'build', '--no-link', '--no-update-lock-file', '--max-jobs', '1', '--cores', '2', *members],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
else:
    assert operation == 'cold'
os.environ.update(SIMIT_NIX_BUILD_RESULTS=str(root), INSTALLABLES=json.dumps(members),
                  SIMIT_NIX_FRESH=json.dumps(members))
try:
    exec(compile(group, '<exact-group-executor>', 'exec'), {'__name__': '__main__'})
except SystemExit as result:
    assert result.code == 0, 'Fresh qualification execution failed'
receipt = json.loads((root / 'receipt.json').read_text())
assert receipt['complete'] and receipt['success']
assert len(receipt['results']) == len(members) and all(row['fresh_execution'] for row in receipt['results'])
for index, name in enumerate(['runtime', 'sibling']):
    assert f'SIMIT_FRESH_TRANSPORT_BUILDER {name}' in (root / str(index) / 'fresh-build.log').read_text()
(root / 'hosted-proof.json').write_text(json.dumps({'head': receipt['head'], 'run': receipt['run'],
    'attempt': receipt['attempt'], 'state': operation, 'dependency_restored': output,
    'current_worker_builders': ['runtime', 'sibling']}, indent=2) + '\n')
