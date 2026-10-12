"""Execute every enrolled output in one store; cached dependencies are shared."""
import json
import os
import re
import subprocess
from pathlib import Path

root = Path(os.environ['SIMIT_NIX_BUILD_RESULTS'])
members = json.loads(os.environ['INSTALLABLES'])
plan = json.loads(os.environ['SIMIT_NIX_PLAN'])
systems = json.loads(os.environ['SIMIT_NIX_CHECK_SYSTEMS'])
limits = json.loads(os.environ['SIMIT_NIX_LIMITS'])
post_build = json.loads(os.environ['SIMIT_NIX_POST_BUILD'])
post_build_always = json.loads(os.environ['SIMIT_NIX_POST_BUILD_ALWAYS'])


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def hooks(scripts, folder, installable):
    env = dict(os.environ, INSTALLABLE=installable, SIMIT_NIX_BUILD_RESULTS=str(folder))
    return [subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', script], env=env).returncode
            for script in scripts]


head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], text=True).strip()
expected = os.environ['GITHUB_SHA']
if os.environ['GITHUB_EVENT_NAME'] == 'pull_request':
    expected = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['pull_request']['head']['sha']
require(head == expected, 'Group source is not the enrolled event head')
subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--'], check=True)
require(members and len(members) == len(set(members)) and set(members) <= set(plan),
        'Group members are empty, duplicated or outside the enrolled plan')
save(root / 'installables.json', members)
receipt = {'schema': 'simit.nix-group.v1', 'head': head, 'tree': tree,
           'run': os.environ['GITHUB_RUN_ID'], 'attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
           'event': os.environ['GITHUB_EVENT_NAME'], 'complete': False, 'results': []}
save(root / 'receipt.json', receipt)
coverage = {}
for system in systems:
    checks = json.loads(subprocess.check_output([
        'nix', 'eval', '--json', '--no-update-lock-file', f'.#checks.{system}',
        '--apply', 'checks: builtins.mapAttrs (_: value: value.drvPath) checks'], text=True))
    require(isinstance(checks, dict) and checks, 'Check inventory is empty or invalid')
    for name, derivation in checks.items():
        attr = name if re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*', name) else json.dumps(name)
        coverage[f'.#checks.{system}.{attr}'] = derivation
save(root / 'coverage.json', {'head': head, 'tree': tree, 'plan': plan, 'checks': coverage})
require(not set(coverage) - set(plan), 'Discovered flake checks have no execution owner: ' + repr(sorted(set(coverage) - set(plan))))
failed = False
for index, installable in enumerate(members):
    folder = root / str(index)
    folder.mkdir()
    (folder / 'installable').write_text(installable + '\n')
    with (folder / 'build.log').open('wb') as log:
        result = subprocess.run(['nix', 'build', '--no-link', '--no-update-lock-file',
                                 '--print-build-logs', '--json', *limits, installable],
                                stdout=subprocess.PIPE, stderr=log)
    (folder / 'result.json').write_bytes(result.stdout)
    valid = False
    if result.returncode == 0:
        try:
            outputs = json.loads(result.stdout)
        except (ValueError, UnicodeDecodeError):
            outputs = None
        valid = bool(outputs) and isinstance(outputs, list) and all(
            isinstance(row, dict) and isinstance(row.get('drvPath'), str)
            and row['drvPath'].startswith('/nix/store/') and row['drvPath'].endswith('.drv')
            and isinstance(row.get('outputs'), dict) and row['outputs']
            and all(isinstance(path, str) and path.startswith('/nix/store/') for path in row['outputs'].values())
            for row in outputs)
    success_hooks = hooks(post_build, folder, installable) if result.returncode == 0 and valid else []
    diagnostic_hooks = hooks(post_build_always, folder, installable)
    save(folder / 'diagnostics-complete', diagnostic_hooks)
    row = {'installable': installable, 'exit_status': result.returncode,
           'output_map_valid': valid, 'evidence': str(index),
           'post_build_statuses': success_hooks, 'post_build_always_statuses': diagnostic_hooks}
    receipt['results'].append(row)
    save(folder / 'status.json', row)
    failed |= result.returncode != 0 or not valid or any(success_hooks + diagnostic_hooks)
    save(root / 'receipt.json', receipt)
    print(json.dumps(row), flush=True)
receipt['complete'] = len(receipt['results']) == len(members)
receipt['success'] = receipt['complete'] and not failed
save(root / 'receipt.json', receipt)
raise SystemExit(1 if failed else 0)
