# Phase 01 — Commit and release simit 0.15.1

> **Recommended Codex model: GPT 5.5 medium**
>
> Moderate complexity, leaf/sub-agent role. The generator change and
> regression test are already in the working tree and tests are green;
> the work is staging, writing a CHANGELOG entry, bumping `version`,
> running the simit release chaperone, and pushing a signed tag. A
> smaller tier could plausibly execute the mechanical pieces but would
> fumble the CHANGELOG framing and the chaperone error triage; a higher
> tier wastes cost on a routine release.

## Working tree

`/data/nvme0/can/Projects/simit` (the simit repo itself).

## Goal

simit 0.15.1 is tagged, pushed, and published to crates.io. Its
generator no longer emits the buggy `grep -m1 -o` extractor in any
generated publish workflow. The CHANGELOG explains what broke for
workspace consumers and points at the research dossier.

## Why this matters now

The detritus workspace publish (the original failure report) is blocked
behind this release. Until simit ships the fix, downstream regen sweeps
(Phase 04) can only use a local checkout, which is fine for one or two
repos but isn't a path users can repeat months later when memory fades.

Originating failure log:

```
Tag 0.1.0 does not match Cargo.toml package version 0.1.0
0.1.0
0.1.0
⚙️ [runner]: exitcode '1': failure
```

The change is one line plus a regression test; both already pass locally.

## Out of scope

- Package-scoping the extractor (that is Phase 02; keep this release
  narrow so the CHANGELOG is short and the diff is reviewable).
- Touching any downstream consumer (Phase 03 and Phase 04 own that).
- Reorganising `validate_release_tag_step` callers — the function
  signature does not change in this phase.

## Plan

1. From the simit repo, verify the working tree contains exactly the
   intended changes:
   ```sh
   git -C /data/nvme0/can/Projects/simit status --short
   git -C /data/nvme0/can/Projects/simit diff src/render/ci.rs tests/init_ci.rs
   ```
   Expect only the one-line generator change and the regression-test
   assertion block (and the planning docs under `docs/src/planning/`).
2. Run the full validation gate:
   ```sh
   cd /data/nvme0/can/Projects/simit
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- --deny warnings
   cargo test --all-features
   cargo package --list
   ```
3. Update `CHANGELOG.md` with a 0.15.1 entry naming the workspace bug
   and linking the research dossier. One paragraph; do not narrate
   internal refactors.
4. Bump `Cargo.toml` `[package].version` from `0.15.0` to `0.15.1`.
   Refresh `Cargo.lock` (`cargo build` or `cargo check`).
5. Stage and commit. Suggested message:
   `Fix publish-workflow version extractor on Cargo workspaces`
   with a Co-Authored-By trailer if the project conventions require it
   (see `git log --format=%B -n 5`).
6. Run the simit release chaperone if the project uses it
   (`simit release ...` per `RELEASING.md`); otherwise tag and push
   manually:
   ```sh
   git tag -s 0.15.1 -m '0.15.1'
   git push origin HEAD 0.15.1
   ```
   The Forgejo publish workflow (which uses the _new_ fixed generator
   only after it regenerates itself in Phase 04) should still succeed
   here because simit is single-crate — confirm.
7. Watch the publish workflow run; if it fails for any reason other
   than the historical bug, classify and decide whether the failure is
   in scope for this phase.

## Acceptance criteria

- [ ] `git log -n1` on the simit repo shows the generator fix + test
      committed with a clear message.
- [ ] Tag `0.15.1` exists locally and on the remote and is GPG-verified
      by `git verify-tag 0.15.1`.
- [ ] Forgejo Actions publish run for 0.15.1 is green and the crate
      `simit = "0.15.1"` resolves on crates.io (`curl -fsS
    https://crates.io/api/v1/crates/simit/0.15.1`).
- [ ] `CHANGELOG.md` entry under `0.15.1` references the research
      dossier path or summarises the bug in one sentence.
- [ ] `cargo install simit --locked --version 0.15.1` succeeds in a
      throwaway shell.

## Files likely touched

- `src/render/ci.rs` (already modified in working tree)
- `tests/init_ci.rs` (already modified in working tree)
- `CHANGELOG.md` (new 0.15.1 entry)
- `Cargo.toml` (`[package].version`)
- `Cargo.lock` (regenerated)

## Pitfalls

- **Symptom:** publish workflow re-tags `0.15.1` but fails the
  tag/version check. **Cause:** the workflow on the simit repo still
  carries the _old_ extractor at publish time, because regenerating
  simit's own `.forgejo/workflows/publish-crate.yaml` is a Phase 04
  task. **Recovery:** simit is single-crate, so the old extractor still
  produces the right scalar; the workflow should pass. If it doesn't,
  hot-patch the workflow file in-place with the same one-line change
  before retagging.
- **Symptom:** `cargo package --list` warns about uncommitted files.
  **Cause:** planning docs left untracked. **Recovery:** either commit
  the planning directory in a separate commit before the release
  commit, or pass `--allow-dirty` only after auditing that nothing
  ships in the crate archive that shouldn't.
- **Symptom:** `git verify-tag` fails. **Cause:** signing key not
  present in the agent's GNUPGHOME. **Recovery:** stop, surface the
  blocker to the user; do not push an unsigned tag.

## Reference

- Research dossier:
  [`../publish-workflow-version-extraction-research.md`](../publish-workflow-version-extraction-research.md)
- Generator: [`src/render/ci.rs:1648-1699`](../../../../src/render/ci.rs#L1648-L1699)
- Existing release notes for cadence:
  [`CHANGELOG.md`](../../../../CHANGELOG.md), `git log --grep Release`.
- Related phase: [`03-detritus-commit-and-retry-publish.md`](./03-detritus-commit-and-retry-publish.md)
  — can run in parallel; only the user-visible release-note narrative
  links them.
