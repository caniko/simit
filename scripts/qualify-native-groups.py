"""Qualify production engines and generated group steps with real native Nix."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import textwrap


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def output(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


source = Path(__file__).resolve().parent.parent
system = os.environ['EXPECTED_NIX_SYSTEM']
machine = {'arm64': 'aarch64', 'amd64': 'x86_64'}.get(platform.machine(), platform.machine())
kernel = {'Linux': 'linux', 'Darwin': 'darwin'}.get(platform.system())
require(f'{machine}-{kernel}' == system, 'Native hardware mismatch')
require(os.environ['RUNNER_ENVIRONMENT'] == 'github-hosted', 'A fresh hosted worker is required')
require(os.environ['GITHUB_RUN_ATTEMPT'] == '1', 'Only original attempts establish qualification')
require(output('nix', 'eval', '--impure', '--raw', '--expr', 'builtins.currentSystem') == system,
        'Configured Nix system mismatch')
head = output('git', 'rev-parse', 'HEAD', cwd=source)
tree = output('git', 'rev-parse', 'HEAD^{tree}', cwd=source)
event_head = os.environ['GITHUB_SHA']
if os.environ['GITHUB_EVENT_NAME'] == 'pull_request':
    event_head = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['pull_request']['head']['sha']
require(head == event_head, 'Checkout is not the original event head')
root = Path(os.environ['RUNNER_TEMP']) / 'native-group-qualification'
root.mkdir(exist_ok=True)
hashes = []
for name in output('git', 'ls-tree', '-r', '--name-only', head, cwd=source).splitlines():
    hashes.append(f'{hashlib.sha256((source / name).read_bytes()).hexdigest()}  {name}\n')
(root / 'source.sha256').write_text(''.join(hashes))
result = json.loads(output('nix', 'build', '--json', '--no-link', '--no-update-lock-file',
                           '--max-jobs', '1', '--cores', '2', '--option', 'accept-flake-config', 'false',
                           '--option', 'access-tokens', '', '--option', 'substituters', 'https://cache.nixos.org',
                           f'github:caniko/simit/{head}#default', cwd=source))
require(len(result) == 1 and set(result[0]['outputs']) == {'out'}, 'Invalid production inventory')
engine = Path(result[0]['outputs']['out']) / 'bin/simit'
require(engine.is_file(), 'Production engine is missing')
(root / 'production-result.json').write_text(json.dumps(result, indent=2) + '\n')
engine_hash = hashlib.sha256(engine.read_bytes()).hexdigest()
(root / 'engine.sha256').write_text(f'{engine_hash}  {engine}\n')
shutil.copyfile(engine, root / 'simit')
fixture = source / 'tests/fixtures/native-groups'
members = [f'.#checks.{system}.{name}' for name in ['runtime', 'sibling']]
runner = 'macos-15' if system.endswith('darwin') else 'ubuntu-24.04-arm'
policy = f'''[ci]
platform = "github"
provider = "actions"
runtime = "nix"
nix_builds = {json.dumps(members)}
[ci.nix_build]
only = true
capture_results = true
max_parallel = 1
max_jobs = 1
cores = 2
check_systems = ["{system}"]
fresh_installables = {json.dumps(members)}
post_build = ["test -s \\\"$SIMIT_NIX_BUILD_RESULTS/result.json\\\"; printf 'success\\\\n' >> \\\"$SIMIT_NIX_BUILD_RESULTS/hooks\\\""]
post_build_always = ["printf 'diagnostic\\\\n' >> \\\"$SIMIT_NIX_BUILD_RESULTS/hooks\\\""]
[ci.nix_build.groups]
native = {json.dumps(members)}
[ci.nix_build.system_runners]
"{system}" = "{runner}"
'''
# Generate outside the parent Cargo workspace, then execute against its tracked fixture.
generation = root / 'generation'
generation.mkdir()
for name in ['flake.nix', 'flake.lock']:
    shutil.copyfile(fixture / name, generation / name)
(generation / 'simit.toml').write_text(policy)
for args in [[], ['--check', '--diff']]:
    subprocess.run([str(engine), 'init', 'ci', *args], cwd=generation, check=True)
workflow = (generation / '.github/workflows/nix-builds.yaml').read_text()
(root / 'generated-workflow.yaml').write_text(workflow)


def step_script(name):
    # Read only a named renderer-owned block; changes in its shape fail explicitly.
    start = workflow.index(f'      - name: {name}\n')
    end = workflow.find('\n      - ', start + 1)
    block = workflow[start:] if end == -1 else workflow[start:end]
    require('        run: |\n' in block, f'Missing generated run block: {name}')
    script = textwrap.dedent(block.split('        run: |\n', 1)[1])
    require('${{' not in script, f'Unbound workflow expression: {name}')
    return script


guard = step_script('Verify native Nix runner')
bootstrap = step_script('Provision grouped-build Rust engine')
build = step_script('Build Nix group')
fallback = step_script('Retain uncollected group diagnostics')
require(workflow.index('Verify native Nix runner') < workflow.index('Provision grouped-build Rust engine'),
        'Engine realization precedes native admission')
require('Enable hosted KVM' not in workflow, 'This fixture must not provision KVM')
before = [output('nix', 'eval', '--raw', '--no-update-lock-file', member + '.outPath', cwd=fixture)
          for member in members]
require(not any(Path(path).exists() for path in before), 'Cold qualification outputs already exist')
proofs = []
for state in ['cold', 'warm']:
    require(all(Path(path).exists() for path in before) if state == 'warm'
            else not any(Path(path).exists() for path in before), 'Unexpected materialization state')
    evidence = root / state
    evidence.mkdir()
    env = dict(os.environ, SIMIT_NIX_BUILD_RESULTS=str(evidence), SIMIT_NIX_GROUP='native',
               SIMIT_CI_ENGINE_REVISION=head, SIMIT_CI_ENGINE=str(engine),
               GITHUB_ENV=str(evidence / 'github-env'), BASH_ENV='', ENV='',
               NIX_USER_CONF_FILES='/dev/null', NIX_PATH='',
               NIX_CONFIG='experimental-features = nix-command flakes\nmax-jobs = 1\ncores = 2')
    for name, script in [('guard', guard), ('bootstrap', bootstrap), ('build', build), ('fallback', fallback)]:
        with (evidence / f'{name}.log').open('wb') as log:
            subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', script], cwd=fixture, env=env,
                           stdout=log, stderr=subprocess.STDOUT, check=True)
    require((evidence / 'engine-source-revision').read_text().strip() == head, 'Engine pin changed')
    require((evidence / 'engine-executable.sha256').read_text().split()[0] == engine_hash,
            'Generated bootstrap engine differs from production generator')
    receipt = json.loads((evidence / 'receipt.json').read_text())
    require(receipt['head'] == head and receipt['tree'] == tree and receipt['run'] == os.environ['GITHUB_RUN_ID']
            and receipt['attempt'] == 1 and receipt['complete'] and receipt['success'], 'Invalid group receipt')
    require([row['installable'] for row in receipt['results']] == members, 'Member cardinality/order changed')
    for index, name in enumerate(['runtime', 'sibling']):
        row = receipt['results'][index]
        require(row['exit_status'] == 0 and row['output_map_valid'] and row['fresh_execution']
                and row['post_build_statuses'] == [0] and row['post_build_always_statuses'] == [0],
                'Fresh execution or hooks did not pass')
        folder = evidence / str(index)
        require(f'SIMIT_NATIVE_PLATFORM_BUILDER {system} {name}' in (folder / 'fresh-build.log').read_text(),
                'Current-worker rebuild marker missing')
        require((folder / 'hooks').read_text() == 'success\ndiagnostic\n', 'Fallback duplicated hooks')
        require(json.loads((folder / 'materialization-result.json').read_text()), 'Materialization missing')
    proofs.append({'state': state, 'receipt': f'{state}/receipt.json', 'freshBuilders': members})
(root / 'hosted-proof.json').write_text(json.dumps({'head': head, 'tree': tree, 'system': system,
    'run': os.environ['GITHUB_RUN_ID'], 'attempt': 1, 'engineSha256': engine_hash,
    'production': result, 'states': proofs}, indent=2) + '\n')
