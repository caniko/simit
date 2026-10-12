# Installed completion-observer acceptance

**Status: procedure prepared; installed-observer acceptance is not established.**
Candidate tests, generator parity, a successful inert producer, and PR workflow
checks qualify the candidate only. Replacement of a waiting collector requires
the live evidence below from an independently authorized trusted default-branch
installation. Keep the collector and existing required test/build/receipt gates
until the consumer owner accepts that evidence.

## Freeze the installation and authority

The repository owner chooses a same-repository acceptance environment and
authorizes its default-branch installation separately. Record repository name
and numeric ID, default branch, installation commit/tree, Simit source commit,
generator binary checksum, policy, generated workflow checksum, and the existing
branch-protection/ruleset configuration. Never use a candidate PR as authority to
install a privileged observer. Recheck these identities before each drill.

Generate `.github/workflows/producer-custody.yaml` with the exact qualified Simit
binary using `simit init ci`, then `simit init ci --check --diff`. Extract each of
the three inline Python programs from the generated YAML and compare its bytes
with `src/render/ci/custody.py`. Inspect the installed workflow: hosted runners,
contents/actions read and statuses write, contents-read identity job, no checkout,
no producer code execution, no archive extraction, upload before reporting,
`always()` upload/report, 25-minute collection and five-minute upload timeout.
Record all immutable action references. Retain the installation PR and approval.

Candidate publication is outside this procedure. In Simit's current stack,
#48 owns primary-only regeneration/publication preservation, #58 owns the
independent publisher, #54 owns grouping/lanes, and #56 owns dependency transport.
Qualification of their combined generation must preserve each policy and output;
completion custody neither publishes packages nor replaces those owners' gates.

## Reproducible producers

Use two small, same-repository producers named `Custody acceptance A` and
`Custody acceptance B`, with exact workflow paths `custody-acceptance-a.yaml`
and `custody-acceptance-b.yaml` beneath `.github/workflows/`. Each has one hosted
job named `fixture`, only contents-read permissions, and triggers on branch
`push` and `pull_request`. Do not use `workflow_dispatch`: it is not an admitted
producer event. Do not give these producers shared concurrency groups; preserve
both events and case-distinct branches. Their only artifact is `custody-fixture`.

Enroll these names/paths/jobs/artifacts in the acceptance repository's custody
policy, with `producer_attempt = 1` and a prefix of at most 22 UTF-8 bytes. Create
the producers as project-owned workflow templates and generate them through
Simit's `ci.workflow_templates`. Use the immutable upload-artifact action from
the qualified generator. Each producer's Python step reads its own delay from
`vars.CUSTODY_A_DELAY` or `vars.CUSTODY_B_DELAY` (default zero, reject values
outside 0–600 seconds), sleeps, and writes these inert files to runner temp:

- `inert-producer-data.py`: `raise RuntimeError("must remain inert")`;
- `source.json`: repository, event, source head, branch, run ID, attempt and
  SHA-256 of the inert bytes. For PRs take head/ref from the event's
  `pull_request.head`, not `GITHUB_SHA` (which can be a merge SHA).

Upload that directory as `custody-fixture`, failing if no files exist. These
fixtures need no checkout. Retain their exact template bytes at the source head.
Give jobs enough timeout for the delay and upload. Ordinary workflows may run
alongside them; their checks remain authoritative.

Use this template for A; for B replace the workflow name and `CUSTODY_A_DELAY`
with B's equivalents. The upload pin must equal the qualified installation's
pin (the example is the current GitHub-hosted pin).

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

In the acceptance repository's `simit.toml`, configure the two template mappings
and this enrollment before generating and reviewing the installation:

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
operator checkout. Set `REPO`, `BASE`, `HEAD`, `BRANCH`, producer run IDs and
observer run IDs to recorded real values; never copy an example SHA into evidence.

```bash
gh variable set CUSTODY_A_DELAY --repo "$REPO" --body 0
gh variable set CUSTODY_B_DELAY --repo "$REPO" --body 300
# Push a new source commit to BRANCH and open its same-repository PR into BASE.
gh pr create --repo "$REPO" --head "$BRANCH" --base "$BASE" \
  --title 'Installed custody acceptance' --body 'Original-attempt observer drill'
gh run list --repo "$REPO" --commit "$HEAD" --limit 100 \
  --json databaseId,workflowName,event,headSha,status,conclusion,attempt
```

Wait until A's observer has uploaded a waiting receipt and published pending
before B completes. If 300 seconds is insufficient, choose up to 600 and start
with a **new source commit**, retaining the incomplete drill as nonacceptance.
Do not rerun a producer to repair a positive drill. Reverse the variables (A=300,
B=0), push another new commit and repeat. Record variables and actual completion
timestamps; delays alone do not prove completion order.

## Required live matrix

| Drill                       | Stimulus                                                                                                                     | Required provider-backed result                                                                                                                                                                                                                    |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A then B                    | Original A completes while B runs, then B completes                                                                          | First observer uploads `waiting`, publishes pending; later observer captures both originals, uploads, freshly revalidates and publishes success.                                                                                                   |
| B then A                    | Reverse the delay on a fresh head                                                                                            | The same pending-to-success transition with B's completion first.                                                                                                                                                                                  |
| Push versus PR              | Push an enrolled head with an open same-repository PR                                                                        | Distinct event contexts and producer sets; push originals cannot fill PR custody or conversely. Both event streams must actually complete.                                                                                                         |
| Branch isolation            | Push the identical head to two refs, including `custody-case` and `Custody-case`, while other observations queue             | Distinct lowercase-hex concurrency groups and contexts; neither ref loses its last completion to the other queue. Each reaches its own terminal result.                                                                                            |
| PR/base isolation           | Open two PRs from the same head/ref into two enrolled base refs                                                              | Different stable targets/contexts; one PR's successful originals cannot fill the other's missing or failed producer. If the API associates a run ambiguously, it must fail admission, never become branch-only success.                            |
| Source-head targeting       | Use a producer head different from the installation/default-branch head and PR merge SHA                                     | Status POST targets the source head; collector SHA is retained separately. No custody status is credited to the default or merge head.                                                                                                             |
| Upload failure              | Separately authorize a trusted test installation that forces the upload step to fail, then complete fresh original producers | Report runs after upload failure, publishes failure to the admitted source context, never success. Retain the failed upload and report logs.                                                                                                       |
| Producer rerun rejection    | First establish success, then `gh run rerun "$PRODUCER_RUN" --repo "$REPO"`                                                  | Original run ID now has attempt 2; its completion is rejected and failure replaces the earlier success at the **same** context. Preserve attempt-1 and attempt-2 API/log evidence.                                                                 |
| Post-upload rerun           | In a separately authorized trusted test installation, pause before report; rerun an admitted producer after custody upload   | Fresh report-time GET observes attempt/status drift; failure revokes success rather than crediting retained stale bytes.                                                                                                                           |
| Snapshot drift              | Pause after capture in that trusted test installation, then advance the PR base without changing its ref                     | Real run-details responses must show a changed observed base SHA. Report rejects the snapshot and revokes the same stable context. If GitHub does not expose the change, mark this live drill unproven; a synthetic contract is not live evidence. |
| Failure publication reserve | Trusted test installation induces slow/failed report lookups within the declared budget, after an earlier success            | Lookup exhaustion still reaches the independent bounded failure POST before job timeout. Retain timestamps, failed lookup and final status.                                                                                                        |

Fault injection belongs to an independently reviewed, immutable **trusted test
installation**, not producer-controlled code, archives, environment values or
candidate-selected scripts. Preserve the installed program's admission and
report logic; record the exact orchestration-only patch (for example a failing
upload action or a bounded pause immediately before report). Repeat positive
drills on the unmodified qualified installation after restoring it. Testing a
modified installation alone does not qualify the production installation.

For post-upload drills, the operator first records a real successful upload ID
and time, then mutates the real producer/base during the bounded report pause.
Retain the receipt snapshot and subsequent native API responses. Do not edit a
receipt to manufacture snapshot drift. A race missed by the operator is not a
pass. Malformed/missing PR associations have no safely established reporting
target; rejection before receipt is expected, and does not prove revocation of
an arbitrary old target.

## Retain and verify the evidence

Use provider run IDs, not names alone. For each producer and observer retain:

```bash
mkdir -p "$EVIDENCE/$RUN"
gh api "repos/$REPO/actions/runs/$RUN" > "$EVIDENCE/$RUN/run.json"
gh api --paginate "repos/$REPO/actions/runs/$RUN/attempts/1/jobs?per_page=100" \
  > "$EVIDENCE/$RUN/jobs-pages.json"
gh api --paginate "repos/$REPO/actions/runs/$RUN/artifacts?per_page=100" \
  > "$EVIDENCE/$RUN/artifact-pages.json"
gh run view "$RUN" --repo "$REPO" --attempt 1 --log > "$EVIDENCE/$RUN/log.txt"
gh api --paginate "repos/$REPO/commits/$HEAD/statuses?per_page=100" \
  > "$EVIDENCE/$RUN/status-history-pages.json"
```

Save the status history **after each transition**, with capture time, rather than
only the combined latest-status endpoint. Fetch exact job-ID logs and raw ZIP
bytes by admitted artifact ID with `gh api`; retain provider digest and compute
the local SHA-256. For rerun drills also retain jobs/logs for attempt 2. Poll
boundedly for terminal observer runs and retain cancelled/failed predecessors;
their evidence never substitutes for the required observation.

Inspect custody receipts and members in a separate credential-free local audit
environment. The privileged observer must not extract producer ZIPs or execute
their contents. Check original attempt-1 job IDs, enrolled names, repository IDs,
source head/tree, raw workflow paths and source-workflow hashes against native
API/source responses. Bare and ref-qualified paths must match the enrolled file
and the observed head/ref/exact PR; wrong qualifiers must reject. Verify that
archive member hashes match inert bytes, and bounds are present in the receipt.

Independently recompute the context from the receipt's stable `snapshot.target`:

```python
canonical = json.dumps(target, sort_keys=True, separators=(',', ':')).encode()
context = prefix + '/' + target['event'] + '/' + hashlib.sha256(canonical).hexdigest()
assert len(context.encode()) <= 100
```

The head SHA selects the commit-status endpoint and is included separately in
concurrency; `observed_base_sha` is an admission snapshot, not part of this stable
key and not historical base-execution provenance. Confirm repository casing is
normalized while ref casing is exact. Compare concurrency output across every
isolation drill. Use status timestamps and observer logs to prove upload precedes
fresh revalidation and success, and that later failure replaces prior success.
The post-upload diagnostic `report-failure.json` is written after artifact upload;
retain report logs/status history as well as the uploaded capture receipt.

The consumer owner signs off a manifest tying every matrix row to installation,
producer/observer run and attempt IDs, heads, stable contexts, status IDs/states,
artifact IDs/digests, logs and observed API snapshots. Missing, cancelled or
ambiguous rows remain open. Only then propose a separate collector-retirement
change with the exact target-context enrollment and unchanged required native
test/build/consumer receipt gates. Candidate readiness and observer-replacement
readiness must be reported separately.
