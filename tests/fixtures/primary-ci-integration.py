"""Guarded semantic merge adaptations for PR #48's pinned reconciliation API.

Run only after composing #48 with the exact checkout whose source was retained.
These explicit adaptations are part of the integration evidence, not an assertion
that the unadapted branches merge without conflict.
"""
from pathlib import Path

path = Path("src/commands/init_ci.rs")
source = path.read_text()
old = 'crate::monorepo::ci::files(&root, &config, true)?'
assert source.count(old) == 1
source = source.replace(old, 'crate::monorepo::ci::files(&root, &config, !command.primary_only)?', 1)
old = '''                command.check,
                command.diff,
'''
assert old in source
source = source.replace(old, '                &command,\n', 1)
old = 'if !command.check && ProjectConfig::can_persist_ci(&root)? {'
assert source.count(old) == 1
source = source.replace(old, 'if !command.check && !command.primary_only && ProjectConfig::can_persist_ci(&root)? {', 1)
path.write_text(source)
Path("tests/primary_ci_native_scope.rs").write_bytes(Path("tests/fixtures/primary-ci-native-scope.rs").read_bytes())
