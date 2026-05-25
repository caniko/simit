# Release Verify

`simit release verify` runs the read-only release bar and reports every check
before choosing an exit code.

```sh
simit release verify --push-target origin
simit release verify --json --version 0.5.1
```

The text output is deterministic: one line per check, followed by a summary.
Failures and blocked checks include a remediation line naming the command or
upstream action that should produce the missing evidence.

```text
simit release verify
  [pass   ] worktree clean
  [pass   ] simit ci managed (no drift)
  [pass   ] simit flake managed (no drift)
  [pass   ] release trust root present (keys/maintainers.gpg)
  [pass   ] CHANGELOG entry exists for 0.5.1
  [fail   ] crates.io: simit 0.5.1 not yet published
            remediation: publish the crate or verify that this pre-release check is being run before publication
  [pass   ] tag presence: 0.5.1 present locally and on origin
  [blocked] remote secrets: CRATES_IO_API_TOKEN presence not verifiable locally
            remediation: run `simit release secrets` once available; until then verify the remote secret in Forgejo/GitHub settings
summary: 1 fail, 1 blocked
```

## Exit Codes

- `0`: every check passed.
- `1`: at least one check failed.
- `2`: no checks failed, but at least one check was blocked because the
  required evidence cannot be verified locally.

Blocked is distinct from failed. The current remote-secret check is blocked by
design because local code cannot prove whether a Forgejo or GitHub repository
has `CRATES_IO_API_TOKEN` configured.

## JSON Schema

`--json` prints one object:

```json
{
  "command": "simit release verify",
  "results": [
    {
      "check": "worktree clean",
      "status": "pass",
      "message": "worktree clean",
      "remediation": "optional remediation text"
    }
  ],
  "summary": {
    "pass": 1,
    "fail": 0,
    "blocked": 0,
    "exit_code": 0
  }
}
```

`status` is one of `pass`, `fail`, or `blocked`. `remediation` is omitted when
there is nothing to fix.

## Checks

The command checks, in order:

- clean git worktree;
- simit-managed CI without generated workflow drift;
- simit-managed flake support without generated file drift;
- release trust root presence and parseability;
- a released `CHANGELOG.md` entry for `--version` or the selected package
  version;
- crates.io reachability for publishable workspace members;
- local tag presence, plus remote tag presence when `--push-target` is set;
- remote publish secrets, currently reported as blocked.

Non-publishable packages are skipped for the crates.io check.
