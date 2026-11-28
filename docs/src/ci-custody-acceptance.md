# Installed completion-observer acceptance

**Status: procedure prepared; installed-observer acceptance is not established.**
Candidate tests, generator parity, a successful inert producer, and PR workflow
checks qualify only the candidate. Keep the waiting collector and required
test/build/receipt gates until the consumer owner accepts the live evidence
specified below from an independently authorized trusted default-branch
installation.

This procedure covers the default `strict-v1` installation. Explicitly selecting
`sealed-v2` adds the [sealed-custody contract and acceptance matrix](ci-sealed-custody.md).
The change applies prospectively: it neither reclassifies historical failed ZIP
checks nor establishes installed-runtime acceptance.

## Freeze the installation and authority

The repository owner chooses a same-repository acceptance environment and
separately authorizes its default-branch installation. Record repository name
and numeric ID, default branch, installation commit/tree, Simit source commit,
generator binary checksum, policy, generated workflow checksum, and the existing
branch-protection/ruleset configuration. A candidate PR never authorizes
privileged observer installation. Recheck these identities before each drill.

Generate `.github/workflows/producer-custody.yaml` with the exact qualified Simit
binary: run `simit init ci`, then `simit init ci --check --diff`. Extract all three
inline Python programs from the YAML and compare their bytes with
`src/render/ci/custody.py`. Check the installed workflow for hosted runners,
collect-scoped contents/actions/checks read and statuses write, contents-read
identity job with an eligibility gate for unenrolled paths, no checkout,
no producer code execution, no archive extraction, upload before reporting,
`always()` upload/report, 25-minute collection and five-minute upload timeout.
Record all immutable action references. Retain the installation PR and approval.

Candidate publication is out of scope. In Simit's current stack, #48 owns
primary-only regeneration/publication preservation, #58 the independent publisher,
#54 grouping/lanes, and #56 dependency transport. Combined qualification must
preserve each policy and output. Custody neither publishes packages nor replaces
those gates.

## Reproducible producers

Use two small same-repository producers: `Custody acceptance A` and
`Custody acceptance B`, at `custody-acceptance-a.yaml` and
`custody-acceptance-b.yaml` under `.github/workflows/`. Each has one hosted
`fixture` job, contents-read only, and triggers on branch `push` and
`pull_request`. `workflow_dispatch` is not admitted. Do not give these producers a shared
concurrency group, so both events and case-distinct branches are preserved. The
only artifact is `custody-fixture`.

Enroll these names/paths/jobs/artifacts in the acceptance repository's custody
policy with `producer_attempt = 1` and a prefix of at most 22 UTF-8 bytes.
Create the producers as project-owned workflow templates and generate the
producer workflows through Simit's `ci.workflow_templates`. Use the qualified
generator's immutable upload-artifact action. Each producer's Python step reads
its delay from `vars.CUSTODY_A_DELAY` or `vars.CUSTODY_B_DELAY` (default zero,
reject values outside 0–600 seconds), sleeps, and writes these inert files to
runner temp:

- `inert-producer-data.py`: `raise RuntimeError("must remain inert")`;
- `source.json`: repository, event, source head, branch, run ID, attempt and
  SHA-256 of the inert bytes. For PRs take head/ref from the event's
  `pull_request.head`, not `GITHUB_SHA` (which can be a merge SHA).

Upload the directory as `custody-fixture`; fail if empty. No checkout is needed.
Retain exact template bytes at the source head. Set each job timeout to cover
delay and upload. Ordinary workflows may run alongside the fixtures and their
checks remain authoritative.

For B, replace A's workflow name and `CUSTODY_A_DELAY` with B's equivalents.
Use the qualified installation's upload pin; the example shows the current
GitHub-hosted pin.

```yaml
name: Custody acceptance A
on:
  push:
    branches: ["**"]
  pull_request:
permissions:
  contents: read
jobs:
  fixture:
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - name: Write inert source-bound fixture
        shell: python
        env:
          DELAY: ${{ vars.CUSTODY_A_DELAY || '0' }}
        run: |
          import hashlib, json, os, time
          from pathlib import Path
          delay = int(os.environ['DELAY'])
          if not 0 <= delay <= 600:
              raise RuntimeError('invalid acceptance delay')
          time.sleep(delay)
          event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
          pr = event.get('pull_request')
          head = pr['head']['sha'] if pr else os.environ['GITHUB_SHA']
          branch = pr['head']['ref'] if pr else os.environ['GITHUB_REF_NAME']
          root = Path(os.environ['RUNNER_TEMP']) / 'custody-fixture'
          root.mkdir()
          raw = b'raise RuntimeError("must remain inert")\n'
          (root / 'inert-producer-data.py').write_bytes(raw)
          identity = {'repository': os.environ['GITHUB_REPOSITORY'],
                      'event': os.environ['GITHUB_EVENT_NAME'], 'head': head,
                      'branch': branch, 'run': os.environ['GITHUB_RUN_ID'],
                      'attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
                      'payload_sha256': hashlib.sha256(raw).hexdigest()}
          (root / 'source.json').write_text(json.dumps(identity) + '\n')
          print(json.dumps(identity))
      - name: Upload inert fixture
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: custody-fixture
          if-no-files-found: error
          retention-days: 32
          path: ${{ runner.temp }}/custody-fixture
```

Set both template mappings and enrollment in the acceptance repository's
`simit.toml` before generating and reviewing the installation:

```toml
[ci.workflow_templates]
".github/workflows/custody-acceptance-a.yaml" = ".simit/workflows/custody-acceptance-a.yaml.in"
".github/workflows/custody-acceptance-b.yaml" = ".simit/workflows/custody-acceptance-b.yaml.in"

[ci.custody]
repository = "OWNER/REPOSITORY" # replace with the authorized real repository
producer_attempt = 1
status_context = "simit/custody"
[ci.custody.producers."Custody acceptance A"]
workflow = ".github/workflows/custody-acceptance-a.yaml"
jobs = ["fixture"]
artifacts = ["custody-fixture"]
[ci.custody.producers."Custody acceptance B"]
workflow = ".github/workflows/custody-acceptance-b.yaml"
jobs = ["fixture"]
artifacts = ["custody-fixture"]
```

The following examples use **Bash** and `gh` from an independently authorized
operator checkout. Set `REPO`, `BASE`, `HEAD`, `BRANCH`, and producer and
observer run IDs to recorded values. Never copy an example SHA into evidence.

```bash
gh variable set CUSTODY_A_DELAY --repo "$REPO" --body 0
gh variable set CUSTODY_B_DELAY --repo "$REPO" --body 300
# Push a new source commit to BRANCH and open its same-repository PR into BASE.
gh pr create --repo "$REPO" --head "$BRANCH" --base "$BASE" \
  --title 'Installed custody acceptance' --body 'Original-attempt observer drill'
gh run list --repo "$REPO" --commit "$HEAD" --limit 100 \
  --json databaseId,workflowName,event,headSha,status,conclusion,attempt
```

Confirm from receipt, status, and completion timestamps that A's observer
uploaded a waiting receipt and published pending before B completed. If
300 seconds is too short, retain the incomplete drill as nonacceptance and
restart with up to 600 and a **new source commit**. Never rerun a producer to
repair a positive drill. Reverse the variables (A=300, B=0), push another new
commit and repeat. Record variables and completion timestamps: delays alone
do not prove order.

## Required live matrix

| Drill                       | Stimulus                                                                                                                                | Required provider-backed result                                                                                                                                                                                                                                                                             |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A then B                    | Original A completes while B runs, then B completes                                                                                     | First observer uploads `waiting`, publishes pending; later observer captures both originals, uploads, freshly revalidates and publishes success.                                                                                                                                                            |
| B then A                    | Reverse the delay on a fresh head                                                                                                       | The same pending-to-success transition with B's completion first.                                                                                                                                                                                                                                           |
| Push versus PR              | Push an enrolled head with an open same-repository PR                                                                                   | Distinct event contexts and producer sets; push originals cannot fill PR custody or conversely. Both event streams must actually complete.                                                                                                                                                                  |
| Branch isolation            | Push the identical head to two refs, including `custody-case` and `Custody-case`, while other observations queue                        | Distinct lowercase-hex concurrency groups and contexts; neither ref loses its last completion to the other queue. Each reaches its own terminal result.                                                                                                                                                     |
| PR/base isolation           | Open two PRs from the same head/ref into two enrolled base refs                                                                         | Different stable targets/contexts; one PR's successful originals cannot fill the other's missing or failed producer. If the API associates a run ambiguously, it must fail admission, never become branch-only success.                                                                                     |
| Source-head targeting       | Use a producer head different from the installation/default-branch head and PR merge SHA                                                | Status POST targets the source head; collector SHA is retained separately. No custody status is credited to the default or merge head.                                                                                                                                                                      |
| Workflow-definition binding | Use a PR whose raw head lacks or differs from a base-introduced workflow; also exercise a producer that checks out a different revision | Installed observer queries the native executed-file record for each original. Its immutable file URL, retained commit/blob/bytes and workflow hash agree; raw head, checkout logs and today's merge ref never substitute for this proof. Missing/mutable proof fails closed at the admitted source context. |
| Upload failure              | Separately authorize a trusted test installation that forces the upload step to fail, then complete fresh original producers            | Report runs after upload failure, publishes failure to the admitted source context, never success. Retain the failed upload and report logs.                                                                                                                                                                |
| Producer rerun rejection    | First establish success, then `gh run rerun "$PRODUCER_RUN" --repo "$REPO"`                                                             | Original run ID now has attempt 2; its completion is rejected and failure replaces the earlier success at the **same** context. Preserve attempt-1 and attempt-2 API/log evidence.                                                                                                                          |
| Post-upload rerun           | In a separately authorized trusted test installation, pause before report; rerun an admitted producer after custody upload              | Fresh report-time GET observes attempt/status drift; failure revokes success rather than crediting retained stale bytes.                                                                                                                                                                                    |
| Snapshot drift              | Pause after capture in that trusted test installation, then advance the PR base without changing its ref                                | Real run-details responses must show a changed observed base SHA. Report rejects the snapshot and revokes the same stable context. If GitHub does not expose the change, mark this live drill unproven; a synthetic contract is not live evidence.                                                          |
| Failure publication reserve | Trusted test installation induces slow/failed report lookups within the declared budget, after an earlier success                       | Lookup exhaustion still reaches the independent bounded failure POST before job timeout. Retain timestamps, failed lookup and final status.                                                                                                                                                                 |

Inject faults only in an independently reviewed, immutable **trusted test
installation**, never via producer-controlled code, archives, environment values
or candidate-selected scripts. Preserve admission and report logic and record
the exact orchestration-only patch (e.g., a failing upload action or a bounded
pause just before report). Restore the unmodified qualified installation and
repeat positive drills; testing the modified installation alone cannot qualify
production.

For post-upload drills, record the successful upload's real ID and time before
mutating the real producer/base during the bounded report pause. Retain the
receipt snapshot and subsequent native API responses. Never edit receipts to
manufacture drift. A missed race is not a pass. Malformed/missing PR associations
leave no safe reporting target: expect rejection before receipt, not proof of
revocation of an arbitrary old target.

## Retain and verify the evidence

Use provider run IDs, not names alone. For each producer and observer retain:

```bash
mkdir -p "$EVIDENCE/$RUN"
gh api "repos/$REPO/actions/runs/$RUN" > "$EVIDENCE/$RUN/run.json"
# NODE_ID is this retained run.json's native node_id, not a workflow name.
gh api graphql -f query='query($id: ID!) { node(id: $id) {
  ... on WorkflowRun { id databaseId runAttempt event
    checkSuite { databaseId repository { databaseId nameWithOwner } }
    file { path repositoryName repositoryFileUrl run { id databaseId runAttempt } }
  }
} }' -f id="$NODE_ID" > "$EVIDENCE/$RUN/workflow-run-file.json"
gh api --paginate "repos/$REPO/actions/runs/$RUN/attempts/1/jobs?per_page=100" \
  > "$EVIDENCE/$RUN/jobs-pages.json"
gh api --paginate "repos/$REPO/actions/runs/$RUN/artifacts?per_page=100" \
  > "$EVIDENCE/$RUN/artifact-pages.json"
gh run view "$RUN" --repo "$REPO" --attempt 1 --log > "$EVIDENCE/$RUN/log.txt"
gh api --paginate "repos/$REPO/commits/$HEAD/statuses?per_page=100" \
  > "$EVIDENCE/$RUN/status-history-pages.json"
```

Save timestamped status history **after each transition**, not just the combined
latest status. Fetch exact job-ID logs and raw ZIP bytes by admitted artifact ID
with `gh api`; retain the provider digest and compute local SHA-256. For rerun
drills retain attempt 2 jobs/logs too. Bound polling for terminal observer runs.
Keep cancelled/failed predecessors; they cannot substitute for the required
observation.

Audit receipts and members locally in a separate credential-free environment.
The privileged observer must not extract producer ZIPs or execute their contents.
Check original attempt-1 job IDs, enrolled names, repository IDs,
source head/tree and raw workflow paths against native API/source responses.
Separately bind each `workflow_revision` to GitHub's documented
[`WorkflowRunFile`](https://docs.github.com/en/graphql/reference/actions#workflowrunfile)
(an executed workflow file), including repository, parent run, attempt, event,
check-suite identity and an immutable `repositoryFileUrl`. Verify its retained
workflow commit/tree, Git blob checksum, exact workflow bytes and per-producer
`source_workflow_sha256`. Source head and workflow commit may differ.
GitHub documents [`GITHUB_WORKFLOW_SHA`](https://docs.github.com/en/actions/reference/variables-reference)
separately from checkout/event SHAs. Producer-written values, checkout logs and today's mutable merge ref cannot
prove the historical definition; never resolve today's mutable merge ref to
invent a historical definition. Without native proof, leave the live row
unaccepted; custody must not report success. Retain
capture-final proof and report-time logs, including failed/mutable/drifting
proof revocation.

Match bare and ref-qualified paths to the enrolled file and observed
head/ref/exact PR; reject wrong qualifiers. Check member hashes against inert
bytes and confirm the receipt includes bounds.

Independently recompute the context from the receipt's stable `snapshot.target`:

```python
canonical = json.dumps(target, sort_keys=True, separators=(',', ':')).encode()
context = prefix + '/' + target['event'] + '/' + hashlib.sha256(canonical).hexdigest()
assert len(context.encode()) <= 100
```

The head SHA selects the status endpoint and enters concurrency separately.
`observed_base_sha` is an admission snapshot, not part of the stable key or
historical base-execution provenance. Confirm normalized repository casing and
exact ref casing. Compare concurrency output across every isolation drill. Use
status timestamps and observer logs to prove upload precedes fresh revalidation
and success, and later failure replaces success. `report-failure.json` comes
after artifact upload; retain report logs/status history and the uploaded receipt.

The consumer owner approves a manifest tying every matrix row to installation,
producer/observer run and attempt IDs, heads, stable contexts, status IDs/states,
artifact IDs/digests, logs and observed API snapshots. Missing, cancelled or
ambiguous rows stay open. Only after sign-off, propose separate collector
retirement with exact target-context enrollment and unchanged required native
test/build/consumer receipt gates. Report candidate and observer-replacement
readiness separately.
