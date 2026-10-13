"""Execute the exact executor assets on fresh hosted workers with real Nix."""
import json
import os
import subprocess
import sys
from pathlib import Path

source = Path(__file__).resolve().parent.parent


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


program = (source / 'src/render/ci/nix_dependencies.py').read_text()
os.chdir(source / 'tests/fixtures/nix-transport')
require(os.environ['RUNNER_ENVIRONMENT'] == 'github-hosted', 'Transport qualification requires a fresh hosted worker')
operation = sys.argv[1]
root = Path(os.environ['RUNNER_TEMP']) / 'transport-qualification'
root.mkdir(exist_ok=True)
members = ['.#checks.x86_64-linux.runtime', '.#checks.x86_64-linux.sibling']
os.environ.update(SIMIT_DEPENDENCY_ROOT=str(Path(os.environ['RUNNER_TEMP']) / 'simit-nix-dependencies'),
                  SIMIT_DEPENDENCY_MAX_BYTES=str(64 * 1024 * 1024), SIMIT_NIX_DEPENDENCIES='[".#input"]',
                  SIMIT_NIX_PLAN=json.dumps(members), SIMIT_NIX_CHECK_SYSTEMS='["x86_64-linux"]',
                   SIMIT_NIX_LIMITS='["--max-jobs","1","--cores","2"]',
                   SIMIT_NIX_POST_BUILD='[]', SIMIT_NIX_POST_BUILD_ALWAYS='[]',
                  SIMIT_DEPENDENCY_MODE='export' if operation == 'export' else 'import')
output = subprocess.check_output(['nix', 'eval', '--raw', '--no-update-lock-file', '.#input.outPath'], text=True)
before = Path(output).exists()
(root / 'dependency-before-import.json').write_text(json.dumps({'path': output, 'present': before}) + '\n')
if operation != 'export':
    require(not before, 'Consumer is not a fresh worker with absent transported input')
exec(compile(program, '<exact-dependency-executor>', 'exec'), {'__name__': '__main__'})
if operation == 'export':
    raise SystemExit(0)
require(Path(output).is_file(), 'Exact dependency output was not restored')
if operation == 'warm':
    with (root / 'warm-materialization.log').open('wb') as log:
        subprocess.run(['nix', 'build', '--no-link', '--no-update-lock-file', '--max-jobs', '1', '--cores', '2', *members],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
else:
    require(operation == 'cold', 'Unknown transport qualification operation')
os.environ.update(SIMIT_NIX_BUILD_RESULTS=str(root))
spec = root / 'group-spec.json'
with spec.open('x') as stream:
    os.chmod(spec, 0o600)
    json.dump({'schema': 'simit.nix-group-spec.v1', 'members': members, 'plan': members,
               'check_systems': ['x86_64-linux'], 'limits': ['--max-jobs', '1', '--cores', '2'],
               'post_build': [], 'post_build_always': [], 'fresh_installables': members}, stream)
# The hosted workflow provisions this exact immutable native engine independently
# before producer bindings. The Python transport adapter remains a separate gate.
subprocess.run([os.environ['SIMIT_CI_ENGINE'], 'ci', 'nix-group', '--spec', str(spec)], check=True)
receipt = json.loads((root / 'receipt.json').read_text())
require(receipt['complete'] and receipt['success'], 'Current-worker qualification is incomplete or failed')
require(len(receipt['results']) == len(members) and all(row['fresh_execution'] for row in receipt['results']), 'Current-worker fresh execution is missing')
for index, name in enumerate(['runtime', 'sibling']):
    require(f'SIMIT_FRESH_TRANSPORT_BUILDER {name}' in (root / str(index) / 'fresh-build.log').read_text(), 'Current-worker builder marker is missing')
(root / 'hosted-proof.json').write_text(json.dumps({'head': receipt['head'], 'run': receipt['run'],
    'attempt': receipt['attempt'], 'state': operation, 'dependency_restored': output,
    'current_worker_builders': ['runtime', 'sibling']}, indent=2) + '\n')
