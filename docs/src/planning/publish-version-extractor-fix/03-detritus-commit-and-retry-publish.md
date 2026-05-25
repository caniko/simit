# Phase 03 — Detritus: commit regenerated workflows and retry publish

> **Recommended Codex model: GPT 5.5 medium**
>
> Moderate complexity, sub-agent role. Three workflow files are
> already regenerated and just need committing on an existing branch,
> followed by a tag/retag decision and re-running the publish
> workflow. The judgement calls (retag in place vs bump to 0.1.1, how
> to express the failure recovery in commit prose, when to abort if
> the publish workflow surfaces a _different_ regression) need real
> reasoning. A `low` tier would mishandle the retag question; `high`
> is unnecessary because no design content is in play.

## Working tree

`/data/nvme0/can/Projects/detritus`, branch `fix/remove-rust-cache`.

## Goal

The three regenerated publish workflows
(`publish-crate-detritus-{client,protocol,server}.yaml`) are committed
on `fix/remove-rust-cache`, pushed, and at least one publish workflow
runs end-to-end successfully against a real release tag for the
detritus crates at `0.1.0`.

## Why this matters now

The detritus 0.1.0 publish is the originating failure. Workflow
regeneration is already in the working tree; the active blocker is
just commit + retry. Until this lands, downstream consumers of any
detritus crate cannot install from crates.io.

## Out of scope

- Bumping detritus crate versions for reasons other than retag
  resolution.
- Restructuring the detritus workspace or its CI/release wiring beyond
  what `simit init ci --platform forgejo --workspace` produced.
- Touching simit (Phase 01 / 02).

## Plan

1. Confirm the local state matches expectations:
   ```sh
   git -C /data/nvme0/can/Projects/detritus status --short
   git -C /data/nvme0/can/Projects/detritus diff .forgejo/workflows/publish-crate-detritus-protocol.yaml
   ```
   Expect three modified `publish-crate-detritus-*.yaml` and a one-line
   `grep -m1 -o` → `grep -o … | head -n1` change in each.
2. Verify each file with the published assertion shape:
   ```sh
   grep -n 'head -n1' /data/nvme0/can/Projects/detritus/.forgejo/workflows/publish-crate-detritus-*.yaml
   grep -n 'grep -m1' /data/nvme0/can/Projects/detritus/.forgejo/workflows/publish-crate-detritus-*.yaml
   ```
   First grep must show three hits; second must show none.
3. Stage and commit:
   ```sh
   git -C /data/nvme0/can/Projects/detritus add .forgejo/workflows/publish-crate-detritus-*.yaml
   git -C /data/nvme0/can/Projects/detritus commit
   ```
   Suggested message:
   `ci: fix publish workflow version extractor (simit-generated)`
   with a body referencing the simit fix commit / dossier.
4. Decide the tag strategy with the user:
   - **Retag in place** — only viable if the existing `0.1.0` tag has
     not been consumed by any external service (crates.io publish
     would have failed). Confirm with
     `git -C ... tag --verify 0.1.0` and a fresh
     `curl -fsSI https://crates.io/api/v1/crates/detritus-protocol/0.1.0`.
     If retagging, delete remote tag, retag the new commit, push.
   - **Bump to 0.1.1** — cleaner audit trail; bump
     `crates/detritus-{client,protocol,server}/Cargo.toml` `version`,
     `cargo build` to refresh lockfile, commit, tag, push.

   Default to bump unless the user explicitly asks for retag.

5. Push the branch and (when ready) merge to `trunk` per project
   convention (this branch is `fix/remove-rust-cache` per dossier
   context).
6. Watch the publish-crate workflows on Forgejo. If any of the three
   fails, classify:
   - Tag/version mismatch → root cause was not what we thought; stop
     and re-investigate.
   - GPG verify failure → reuse the maintainer key bootstrap from
     prior runs.
   - Crates.io 409 already-published → expected for retag; the
     workflow handles this and exits 0.
7. Confirm crates.io now lists all three crates at the chosen version.

## Acceptance criteria

- [ ] `git log -n1` on `fix/remove-rust-cache` shows the workflow
      regen committed with a clear message.
- [ ] All three publish-crate workflows on the published commit
      contain `grep -o … | head -n1` and none contain `grep -m1 -o`.
- [ ] At least one of `detritus-client`, `detritus-protocol`,
      `detritus-server` resolves on crates.io at the chosen version
      (`curl -fsSI https://crates.io/api/v1/crates/<crate>/<ver>`).
- [ ] The publish workflow log no longer shows the
      "Tag X does not match Cargo.toml package version X" line.

## Files likely touched

- `.forgejo/workflows/publish-crate-detritus-client.yaml` (already
  modified in working tree)
- `.forgejo/workflows/publish-crate-detritus-protocol.yaml` (ditto)
- `.forgejo/workflows/publish-crate-detritus-server.yaml` (ditto)
- Possibly `crates/detritus-*/Cargo.toml` and `Cargo.lock` if a
  version bump is chosen.

## Pitfalls

- **Symptom:** publish workflow now fails on the _wrong_-package
  version comparison (workflow read detritus-client's version while
  publishing detritus-protocol). **Cause:** Phase 02 not yet landed;
  extractor still picks first package in `packages[]`. **Recovery:**
  while all three are 0.1.0 the comparison still passes; if a single
  crate is bumped independently before Phase 02 lands, hot-patch the
  affected workflow to use `cargo metadata … --manifest-path
crates/<pkg>/Cargo.toml` or wait for Phase 02.
- **Symptom:** `git push --tags --force` rejected by remote.
  **Cause:** branch protection or maintainer key policy. **Recovery:**
  prefer the bump-to-0.1.1 path; do not push `--force` to shared
  branches without explicit user confirmation.
- **Symptom:** `cargo publish` 403/401. **Cause:** `CRATES_IO_API_TOKEN`
  secret missing or wrong scope. **Recovery:** confirm the secret is
  set at the repository level and the token has `publish-new` and
  `publish-update` scopes for all three crate names.

## Reference

- Research dossier:
  [`../publish-workflow-version-extraction-research.md`](../publish-workflow-version-extraction-research.md)
- Detritus workspace: `/data/nvme0/can/Projects/detritus`
- Originating failure log (paste from CI):
  `Tag 0.1.0 does not match Cargo.toml package version 0.1.0` followed
  by two trailing `0.1.0` lines.
- Related phase: [`01-simit-commit-and-release.md`](./01-simit-commit-and-release.md)
  — runs in parallel; only the release-note narrative links them.
