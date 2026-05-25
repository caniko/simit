# Phase 05 — Surface `conflicted` hook state in `simit projects` output

> **Recommended Codex model: GPT 5.5 medium**
>
> Sub-agent work touching the projects-list formatter and
> potentially `src/cli.rs` for a small flag. The new
> `FeatureStatus::Conflicted` state from phase 01 only delivers
> value if a user running `simit projects list` _notices_ it during
> routine work. Sketch options: ANSI-highlight `conflicted` in the
> table; promote it to a top-level warning line; add a
> `--show-issues` flag; or add a thin `simit doctor` subcommand.
> Picking the right surfacing without making the default output
> noisy is the design content. `low` risks shipping a
> "just colorize it red" implementation that is invisible on
> NO_COLOR terminals; `high` is wasted on a formatter tweak.

## Working tree

`/data/nvme0/can/Projects/simit`.

## Goal

A user running `simit projects list` (or `simit projects show
<path>`) in routine workflow can see at a glance that one of their
projects is in a `conflicted` hooks state — without piping through
`jq` or knowing to ask. The signal must work on a plain
non-color TTY (so the AI-strip-with-sed mindset doesn't beat it).

## Why this matters now

Phase 01 introduces a `conflicted` state precisely because today's
silent mislabeling is what let detritus's CI clippy failure go
undetected for as long as it did. If the new state just sits in
the JSON output that nobody reads, this whole plan set has just
shifted the silent failure from one column to another. Phase 05
closes that loop.

## Out of scope

- A general `simit doctor` framework covering features beyond
  hooks. That's a separate plan if and when other features grow
  conflicted states. This phase is hooks-specific.
- Sending notifications, opening editor buffers, or other
  out-of-band signaling. Output-only.
- Per-project remediation hints in the output. The user has the
  research dossier and `simit hooks install --help` for that.

## Plan

1. **Pick the surfacing mechanism.** Three options:

   **A. Per-row prefix glyph.** Prepend a `!` (or `⚠`) to any
   row in `simit projects list` where any feature is in a `drift`,
   `conflicted`, or other "needs attention" state.
   Pros: zero new flags, always visible. Cons: changes default
   output format (visible breakage for any scraper).

   **B. Footer warning.** After the table, print a blank line and
   `<N> project(s) need attention: <space-separated paths>` if any
   project has a conflicted/drift state. Pros: backward-compatible
   for parsers reading the table. Cons: easy to miss if output is
   long.

   **C. Both.** Glyph in the row, summary in the footer.

   **Recommendation: C, gated on `--issues`/`--no-issues` flag with
   `--issues` as default-on for the human-facing
   `simit projects list` and default-off for any JSON output
   (which already exposes the state field).**

2. **Locate the list formatter.** Likely in `src/commands/projects.rs`
   (or wherever `simit projects list` is implemented). Find:
   - The function that renders the table for human output.
   - The function that renders JSON output (no behavior change
     there).

3. **Define "needs attention".** A project needs attention if any
   feature's status is one of:
   - `hooks: conflicted` (new from phase 01)
   - `ci: drift`, `flake: drift`, `changelog: drift` (existing
     states; surface them too while we're here — same UX
     justification).

   Skip projects with `path` under `/tmp/` and `/tmp/nix-shell.*`
   (ephemeral, already filtered by simit's project sweeping
   conventions per the dependent-fixes skill).

4. **Implement the glyph + footer.** Conservative ANSI: use
   `if stdout.is_terminal() && env::var_os("NO_COLOR").is_none()`
   to gate the optional yellow color on the glyph; the glyph
   character itself appears regardless. Footer line:

   ```text
   2 project(s) need attention:
     /data/nvme0/can/Projects/detritus   hooks=conflicted
     /data/nvme0/can/Projects/skillnet   ci=drift
   ```

5. **Add the `--issues` / `--no-issues` flag.**
   `simit projects list --no-issues` reverts to today's pure-table
   output. Default is `--issues`. Document in `--help` text.

6. **Adjust `simit projects show <path>`.** When the inspected
   project has any "needs attention" feature, print a single
   highlighted line at the top:

   ```
   ⚠ project has 1 issue: hooks=conflicted
   ```

   Below the existing table, with no other format change.

7. **Tests.** Unit-test the "needs attention" classifier in
   isolation. Integration test that runs `simit projects list`
   against a fixture registry with one conflicted project and
   asserts the footer appears.

8. **Run gates.**

   ```sh
   cargo fmt --all -- --check
   cargo test --all-features
   cargo clippy --all-targets --all-features -- --deny warnings
   ```

9. **CHANGELOG.** Add an entry under `[Unreleased]` describing the
   new attention-surfacing behavior and the `--no-issues` opt-out.

## Acceptance criteria

- [ ] `simit projects list` (no flag) on a fixture with at least
      one `conflicted`/`drift` project prints the glyph in the
      affected row(s) AND a footer block listing each issue.
- [ ] `simit projects list --no-issues` produces output
      byte-identical to today's behavior (table only, no glyph,
      no footer).
- [ ] `simit projects list --json` is unchanged (no glyphs, no
      footer in JSON).
- [ ] `simit projects show <conflicted-project>` prints the
      single-line attention header above the existing table.
- [ ] Unit test for the classifier covers: hooks=conflicted →
      attention; flake=drift → attention; hooks=configured → no
      attention; hooks=installed → no attention; all features
      managed/installed → no attention.
- [ ] `cargo clippy --all-targets --all-features -- --deny warnings`
      is clean.
- [ ] `NO_COLOR=1 simit projects list` produces no ANSI escape
      sequences (glyph still appears as a plain character).

## Files likely touched

- `src/commands/projects.rs` (or equivalent) — formatter changes,
  classifier, flag.
- `src/cli.rs` — new `--issues` / `--no-issues` flag on the
  `projects list` and `projects show` subcommands. **Note:**
  phase 02 also touches this file; rebase carefully — different
  enum variants but the same file.
- `tests/projects_list_issues.rs` — new integration test.
- `CHANGELOG.md`.

## Pitfalls

**P1. ANSI escape sequences in piped output.** Symptom: a user
runs `simit projects list | less` and sees `^[[33m⚠^[[0m`. Cause:
color was not gated on `is_terminal()`. Recovery: gate strictly;
plain ASCII glyph survives the pipe just fine.

**P2. The glyph character renders as a box on some terminals.**
Symptom: `⚠` shows as `□` or `?`. Cause: terminal font lacks the
codepoint. Recovery: prefer `!` (ASCII) as the primary glyph; use
`⚠` only when `LANG` or `LC_ALL` contains `UTF-8` and a TTY check
passes. Or just use `!` always — simplest.

**P3. `simit projects list` output drift breaks downstream
scripts.** Symptom: a script that greps the output starts seeing
extra lines. Cause: the footer is new. Recovery: this is why
`--no-issues` exists; document it as the stable-for-scripts
interface. Mention in CHANGELOG.

**P4. Classifier disagreement between `list` and `show`.**
Symptom: `list` flags a project that `show` doesn't, or vice
versa. Cause: classifier reimplemented in two places. Recovery:
extract a single `pub fn needs_attention(features: &Features)
-> Vec<AttentionItem>` and reuse.

**P5. CI / drift detection itself is fragile.** Symptom: `flake:
drift` appears for projects that are actually fine because simit's
drift detection had a false positive. Cause: pre-existing simit
bug, out of scope for phase 05. Recovery: file a follow-up; phase
05 just surfaces the state simit reports — it does not relitigate
detection correctness for non-hooks features.

## Reference

- Research dossier: [hooks-enforcement-research.md](./hooks-enforcement-research.md)
- Phase 01 (prerequisite — introduces the `Conflicted` state):
  [01-simit-detect-hooks-status.md](./01-simit-detect-hooks-status.md)
- `is_terminal` crate or std method:
  <https://doc.rust-lang.org/std/io/trait.IsTerminal.html>
- NO_COLOR convention: <https://no-color.org/>.
