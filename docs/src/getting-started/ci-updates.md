# Workstation CI updates

The dev-station flake sets which repositories may receive generated CI updates,
the qualified generator revision, forge connections and rollout limits. Simit
applies that policy in disposable clones and keeps persistent receipts.

## NixOS configuration

Import `inputs.simit.nixosModules.updater`. The executor defaults to that input's
package; each profile enrolls its own generator executable and source revision.
For standalone installations, `inputs.simit.lib.mkUpdateConfig` renders the same
versioned policy.

```nix
{
  imports = [inputs.simit.nixosModules.updater];

  services.simit-updater = {
    enable = true;
    user = "developer";
    schedule = "hourly"; # null leaves services available for manual execution

    # Git is the only default runtime tool. Choose additional tools according
    # to the generator, adapters, transport and signing policy in this profile.
    runtimePackages = with pkgs; [git openssh cargo nix sqlite gnupg];
    sandboxPackage = pkgs.bubblewrap;

    workspaces.projects = {
      root = "/work/projects";
      stateDirectory = "/var/lib/project-ci-updates";
      inventory = {
        command = {
          program = "/opt/workspace/bin/inventory";
          arguments = ["--format" "simit-update-v1"];
        };
        maxAgeSeconds = 300;
      };
    };

    forges.github = {
      host = "github.com";
      apiUrl = "https://api.github.com";
      credentialFile = "/run/credentials/simit/github-token";
      checkAuthority = "15368"; # explicitly enrolled GitHub Actions app ID
      adapter = {
        program = "${inputs.simit.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/simit";
        arguments = ["projects" "update-github-adapter"];
      };
    };

    profiles.fleet = {
      workspace = "projects";
      generator = {
        executable = "${inputs.qualified-simit.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/simit";
        sourceRevision = inputs.qualified-simit.rev;
        sourceCoordinate = "github.com/example/simit";
        forge = "github";
        requiredChecks = [{name = "test"; authority = "15368";}];
      };
      select = {
        coordinates = ["github.com/example/canary" "github.com/example/project"];
        kinds = ["owned-repo"];
        lifecycles = ["active"];
        access = ["normal"];
        checkoutRoles = ["canonical"];
        includePaths = ["**"];
        excludePaths = ["retired/**"];
      };
      # Explicitly admit the first managed baseline. Later updates compare
      # managed files with the last successfully adopted runtime baseline.
      enrollCurrent = true;
      requiredChecks = [{name = "test"; authority = "15368";}];
      rollout = {
        canaries = ["github.com/example/canary"];
        batchSize = 5;
        maxConcurrent = 1;
        maintenanceRefPrefix = "simit/update/";
      };
      projectOverrides."github.com/example/custom-ci" = {
        enable = false;
        reason = "Independent project-owned CI";
        # Optional kebab-case code for tooling; only on disabled overrides.
        blocker = "custom-ci";
      };
      # Protected targets that require pull requests: the updater opens and
      # merges one instead of pushing. Profiles take the same `landing` option.
      projectOverrides."github.com/example/reviewed" = {
        landing = "pull-request";
      };
    };
  };
}
```

The operator sets the identities, paths, endpoints, authorities and checks.
Use the qualified generator source and the selected repositories' actual
required check names. A non-store generator also needs `executableSha256`.
Scripts need an immutable Nix-store interpreter chain: no `/usr/bin/env` or
mutable system interpreters. Native executables must be store-backed even with
a digest, which alone cannot bind dynamic loaders or shared libraries.

Runtime roots and credential paths are strings. Nix evaluation never inspects
worktrees, queries forges or reads credentials. Git transport uses the execution
user's existing Git configuration, and candidate commits honor that user's
identity, signing and hooks. `credentialFile` authenticates API reads and, only
for pull-request landing, opens, merges and closes pull requests. Fine-grained
GitHub tokens then need `Pull requests: write` and `Contents: write` on enrolled
repositories.

The sandbox has no personal Simit configuration. Set the platform and private
runner labels in project CI policy; GitHub can use hosted defaults.
Without a platform, Simit defaults to Forgejo, where release plans fail without
`[release.artifacts].runner` or `[ci].runner`. If a run changes CI workflows, the
flake or other existing outputs, or removes files, the `readme` scope skips the
README. The next run regenerates badges from the landed tree.

The module creates `simit-updater-<profile>.service` and, if scheduled, a timer.
Each service can write only its profile's private workspace state directory;
workspace source and user configuration stay read-only. Use an existing user
or the module's `simit-updater` user.
Provision its Git identity, signing, transport and credential-file access.
Services load the regular Nix-store policy file directly; operators use its
`/etc/simit/updates.json` link.

Profiles sharing a workspace acquire its exclusive updater lock before discovery
or transactions and verify the lock file's identity after waiting. State
directories must be disjoint even after runtime symlink resolution. Enrollment
digests require exactly 64 lowercase hex characters; generator source revisions
require full 40-character lowercase commit SHAs.

Inventory paths, command executables and credential files use absolute runtime
paths. Rust and Nix reject store credential paths and credential/code overlaps
without filesystem reads. At runtime, credentials must resolve outside the store.
Reads verify the opened regular file's identity through Linux procfs and snapshot
bytes from the same descriptor.

## Selection and inventory

Set exactly one of `inventory.file` or `inventory.command`. Commands need absolute
executables, argv arrays and timeouts of 1 through 900 seconds. Inventory
freshness ranges from 1 through 86400 seconds. Both inventory sources supply a
JSON snapshot of this shape:

```json
{
  "schemaVersion": 1,
  "root": "/work/projects",
  "observedAt": "2026-10-17T06:00:00Z",
  "producer": "workspace-inventory.v1",
  "policyDigest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "projects": [
    {
      "coordinate": "github.com/example/canary",
      "forge": "github",
      "relativePath": "any/configured/layout/canary",
      "kind": "owned-repo",
      "lifecycle": "active",
      "access": "normal",
      "checkoutRole": "canonical",
      "simitCi": "auto",
      "mirrors": []
    }
  ]
}
```

The workstation operator must set a non-empty `select.coordinates` list per
profile. Inventory cannot extend enrollment: `policyDigest` records provenance,
not deployment authority. The updater rechecks exact coordinates when processing
a frozen selection. Nix and runtime validation reject canaries outside
enrollment before discovery.

Configure exactly one policy per forge hostname. Inventory can name it, not
substitute a weaker adapter, credential or check authority. Rust and Nix reject
duplicate hosts before discovery. Forge entries and enrolled or discovered
coordinates require lowercase hostnames. Validation rejects case aliases
before discovery rather than rewriting enrollment or receipt keys.
Remote DNS host matching ignores case; repository owner/path spelling is exact.

Inventory commands run in Linux filesystem, network and PID namespaces with
read-only mounts, a private writable temp directory and no host `/run` socket
tree or Nix daemon socket. Descendants cannot outlive discovery. The persistent-state
checkpoint covers all persistent updater state during discovery with no state
subtree excluded, detecting and restoring unexpected writes before inventory
output reaches a transaction, even on failure.

The policy path can be a symlink (e.g. `/etc/simit/updates.json` on NixOS) to a
target that passes the regular-file JSON reader. State and evidence readers
still enforce strict regular-file checks.

The timestamp must be fresh at rollout start. Metadata selectors combine with
AND, path exclusions win, and exact coordinates disambiguate short names.
In relative path globs, `*` cannot cross `/`, but `**` can. The Nix renderer
accepts literal components, `?`, `*` and whole `**` components. It rejects
malformed recursive globs and bracket classes before generating policy.
Worktrees and mirrors are observations, not separate default targets. Duplicate
selected canonical identities fail, and `custom` and `excluded` CI policies
are ineligible. Forge connections and eligible profile selections together
determine deployment authority, not inventory alone.

Adapters can reuse workspace catalogs. Canix's
`workspace simit-inventory --raw --forge HOST=KEY` uses its configured tree,
sources, exclusions and canonical/mirror policy. Select its `observed` lifecycle
explicitly, or use a producer with independently declared lifecycle data.

## Commands and transaction

```sh
simit projects update --profile fleet --plan --json
simit projects update --profile fleet --dry-run
simit projects update --profile fleet --push
simit projects update --profile fleet --status
simit projects update --profile fleet --retire github.com/example/demo
```

`--plan` explains each enrolled coordinate, including missing inventory, disabled
overrides, CLI narrowing and exclusions by kind, lifecycle, access, checkout role,
CI policy or path. Versioned JSON retains inventory and policy digests and marks
qualification `not-run`. The plan runs protected discovery under the workspace
lock but never runs forge adapters, reads forge credentials, verifies generators,
clones repositories or creates candidates or rollout progress.

Plans report empty selections and canaries rejected by lifecycle or other
filters. Execution requires every canary to be eligible and survive CLI
narrowing. Duplicate eligible canonical identities, unlisted canaries and invalid
CLI selections still fail. `--plan` reports the full selection before batching;
it cannot run with execution/status modes or a batch limit.
Disabled entries carry the override's `reason` and optional `blocker`, a
kebab-case grouping code (`[a-z][a-z0-9-]*`). Rust and Nix reject `blocker` on
enabled overrides.

Outside NixOS, use `--config PATH`. Narrow selection with `--repo HOST/OWNER/REPO`
without dropping configured canaries. `--limit N` further caps batch size.
Repeat `--push` to resume the recorded rollout and candidate commit. Nix limits
`batchSize` and `maxConcurrent` to `1..4294967295`, matching the runtime's
unsigned 32-bit representation.

The executor freezes selected identities and processes canaries first. A failed
or incomplete canary stops the batch even with `stopOnFailure = false`. Execution
is serial: `maxConcurrent` is a ceiling, not a parallelism guarantee. Each
invocation takes up to `batchSize` repositories from the same snapshot.

A failure before writing the candidate receipt pushes nothing and recurs on
retry with the same source, generator and policy. With `--push`, the updater
reports `refused` and the error, drops the repository from the frozen rollout
and continues the batch, but exits non-zero. Canaries never drop out this way.
A repository with an unfinished receipt (a pushed or parked candidate) stays
`failed` and keeps its place, subject to `stopOnFailure`. The next rollout
selects refused repositories again when it freezes a new selection.

`--retire HOST/OWNER/REPO` abandons one unfinished candidate in the specified
profile. It deletes the maintenance ref under a lease on the candidate commit
(also closing any open pull request for that branch), archives the receipt as
`invalidated-<digest>.json` beside the adopted baseline, and drops the coordinate
from the profile's unfinished frozen rollouts so the next run generates a fresh
candidate. A canary stays and gates the rollout again. Rollouts frozen before
the profile was recorded stay untouched while another profile holds an unfinished
candidate for that repository.

`--retire` refuses an `adopting` candidate or one reachable from the default
branch (retirement would leave the baseline out of sync); resume it instead.
Retirement leaves adopted receipts untouched and does nothing when repeated. If
ref deletion fails, the command fails and keeps the receipt. A coordinate with
no receipt in the profile requires `--prune-refs`. Pruning deletes
candidate-shaped refs (`maintenanceRefPrefix` plus the first 16 hex digits of
their commit) only if no profile's unfinished receipt for that repository names
them; all other refs under the prefix are never touched. Pruning uses the
retained checkout to reach the remote, so run `--dry-run` first if the updater
has never cloned the repository.

For each repository:

1. Resolve forge identity, remote default branch, target revision and protection.
2. Qualify the pinned generator source and verify its enrolled executable.
3. Generate one versioned plan per enrolled scope ([Scopes](#scopes)) in an
   isolated Linux sandbox. Plans list writes and deletions using the project's
   resolved configuration, without creating a config file. Generation must
   preserve persistent updater state, Git configuration, hooks, refs and source
   files. Checkpoint state outside the checkout and verify it before trusting
   the plan, even on failure. Restore any mutations and fail the transaction.
4. Compare managed files with the adopted baseline, prepare a scoped commit and
   retain its diff and identity. The plan must keep or explicitly mark obsolete
   every existing managed path; omission cannot drop ownership.
5. Admit the exact plan for both maintenance and adoption branches, even on
   resume. Push the maintenance ref and qualify the exact candidate's original
   push.
6. Reinspect source and policy and repeat target admission. Conditionally
   advance the target to the same candidate under an explicit Git lease, or
   propose and merge a pull request with `landing = "pull-request"` (see below).
7. Verify exact-source default/target-branch adoption, record a new baseline and
   delete the maintenance ref under a lease on the candidate.

Managed identities use the canonical Git mode and blob from the staged/committed
tree, so clean/smudge filters and newline conversion don't invalidate the next
rollout. Reconcile executable-bit changes even if content is unchanged.
Working-copy checks honor `core.fileMode`. Explicitly reconcile older blob-only
baselines (they lack mode evidence) and adopted receipts missing a baseline.
`enrollCurrent` cannot silently recreate a missing baseline. Serialized state
must fit the reader's size limit before replacing a retained receipt.

Generation exposes only the read-only candidate checkout, enrolled executable
and Nix store (via a disposable copy-on-write overlay). The child gets a cleared
environment, private temporary home, and network and PID namespaces. It cannot
access updater state, credential files, the SSH agent, the host Nix daemon or
the host fetcher cache. Its PID-1 lifetime kills and reaps descendants before
success. The dev-station flake can override native `sandboxPackage`; standalone
installations must supply immutable store-backed `bwrap` and runtime tools on
`PATH`. Missing namespace support fails closed.

If AppArmor restricts user namespaces, allow the enrolled immutable Bubblewrap
executable in host configuration. The updater doesn't change host policy.
Hosted qualification records the initial probe and kernel diagnostics, loads
an executable-scoped allowance on its disposable runner, then probes again
with isolation still enabled.

Flake configuration forbids input/lock updates, flake-provided Nix settings
and import-from-derivation. Evaluation uses an offline disposable local store
initialized from a consistent SQLite snapshot of host Nix store registrations
and the schema marker. The snapshot includes committed WAL contents; generation
never sees the live database or daemon. `NIX_STATE_DIR` selects the host state
directory (default `/nix/var/nix`). Registered locked inputs are available
offline; explicitly prepare missing inputs rather than fetching with credentials
during generation. The updater currently requires Linux. The updater's NixOS module supplies
SQLite; standalone flake-backed configurations need immutable `sqlite3` on `PATH`.

Workflow branch filters must admit `maintenanceRefPrefix`. Generation appends
`simit/update/**` to non-empty GitHub `push_branches` filters that don't
already match it. Crow `build` and `ci` workflows admit it alongside
`${CI_REPO_DEFAULT_BRANCH}` for push pipelines. List non-default prefixes in
`push_branches`; Crow doesn't run push pipelines on those maintenance refs.
Missing candidate events, checks or capabilities block landing. Remote
movement, default-branch or protection changes, and managed-file edits
invalidate prior acceptance. Failed, skipped, cancelled, neutral or rerun
required cases cannot qualify.

`--status` reads receipts and adapter responses from `stateDirectory` without
discovery or forge queries. Receipts record the producing `generatorRevision`;
receipts predating that field report `null` until their next transaction.

Each profile has its own directory in `stateDirectory` for clones, receipts
and baselines. `--status` reports only the requested profile. Receipt policy
hashes bind the active profile, workspace root and state directory, and
generator and consumer forge records. In `generator.executable`,
`adapter.program` and adapter arguments, `/nix/store/<hash>-<name>` contributes
only `<name>`. Generator identity uses `sourceRevision`, `sourceCoordinate`
and any `executableSha256`; adapter identity uses the command, `adapterSha256`
and `adapterInputs`. The hash excludes the inventory source; frozen selections
bind observations by `inventoryDigest`.

Rebuilding the host at the same generator revision preserves unfinished
candidates and frozen rollouts. Changing the generator revision, adapter, forge
record or required check still invalidates them. This version writes
`schemaVersion: 2` receipts, which also record `projectPolicyDigest` (the same
hash minus other coordinates' enrollment, canary membership and overrides).
Unfinished v2 receipts resume against it, so changes only to other coordinates'
enrollment, canary membership or overrides keep the candidate or pull request.
Receipts without `schemaVersion` are v1: they resume against
`configDigest` and land directly. Older executors reject v2 receipts.

Frozen selection keys use profile, workspace and generator forge policy.
Unrelated edits preserve frozen selections and resumable candidates; changes
to referenced policy invalidate pending receipts. Frozen projects revalidate
against the currently enrolled consumer forge host before any adapter request.

If source moves before landing, the updater archives the invalidated receipt,
deletes the maintenance ref and rebuilds from the new source without changing
the adopted baseline. If the ref is already deleted or was moved by another
writer, cleanup logs it and continues without failing or deleting it. Explicitly
reconcile adoption if the candidate already awaits adoption or is an ancestor of
the new source; the updater won't silently restart.

Interrupted generation or discovery, or failed checkpoint restoration, leaves
`generation-checkpoint.json` in the workspace state root. It names the recovery
snapshot in the same durable directory. The updater syncs recovery bytes before
publishing the marker; service-private temporary storage holds only disposable
tool output. Further transactions stop before adapter dispatch. Stop the
workspace's updater services and reconcile state against the snapshot before
removing the marker. The snapshot excludes the current disposable checkout but
preserves other state files and permissions.

The updater verifies the workspace lock's file identity across generation and
before saving rollout progress. If the lock file's identity changes, the updater
stops and keeps recovery evidence rather than restoring under a different lock.

Provision each service state directory as mode `0700` and owned by its configured
user. The module won't create arbitrary state paths through root-owned tmpfiles.
Before creating files, the updater resolves existing path components and rejects
any state tree overlapping a resolved workspace root.

Forge adapters run in Linux namespaces with an empty filesystem view populated
only by the read-only Nix store, enrolled code snapshots, exact request,
configured credentials, DNS plumbing and immutable CA certificates. They retain
network access to their forge and get private home and temporary storage, no
host state or daemon sockets. Descendants cannot outlive the adapter.

Forge adapters need a store-backed executable or `adapterSha256`. File-valued
configured arguments must be absolute store paths or path-to-SHA-256 entries in
`adapterInputs`. Each verification reads or executes digest-enrolled mutable
files from fresh private snapshots and retains prior verified bytes, even for
re-enrolled or repeated inputs. Store inputs must resolve to files inside the
store. Adapters and generators keep their enrolled store paths even with a
digest, preserving wrapper resources and relative loaders. Containment checks
resolve symlinks but preserve invocation names that may select multicall
applets.

The updater checks every `adapterInputs` entry, even helpers absent from argv.
Enroll the executable's digest in `adapterSha256`, not `adapterInputs`. Before
dispatch, Rust and Nix reject duplicate entrypoint paths (including repeated
separators) and credential paths overlapping the executable or any declared
input. Adapters run from a private directory and get runtime values via request
JSON. The sandbox mounts verified mutable snapshots at enrolled absolute paths
so scripts can import declared siblings without exposing host originals or
undeclared mutable helpers. The same checks apply to store interpreters
executing configured script arguments. Script interpreter chains must resolve
entirely inside the store. Shebangs take no arguments, Python's isolated `-I`,
or shell `-e`; arguments that can select another script or loader fail. Tool
execution clears runtime-injection variables and disables Python user-site
imports. Python scripts should use isolated (`-I`) store interpreters. Inventory
launchers clear the environment and apply an explicit runtime allowlist.
Adapters use only store-backed `PATH`/CA paths and private home/temp settings.
Forge/package-registry credentials and unlisted variables are excluded. The only
credentials supplied to an adapter are its configured `credentialFile` and
`ciCredentialFile`, identified in request JSON and mounted read-only.

Separate Git transport commands retain operator identity, signing and transport
configuration. Every updater Git command clears repository-routing variables
such as `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, object directories and ref
namespaces, preventing redirection outside the disposable clone. HTTPS clone
URLs cannot contain usernames or passwords; SSH usernames use operator Git
authentication. Clone URLs must use the protocol's default port because
enrollment identifies the service by hostname. The updater validates override
target branches as complete Git branch refs before discovery; Nix enforces the
same grammar.

Generator qualification selects push workflows with configured required check
names under their enrolled app authority. Both the API query and admitted
original runs bind the generator repository's current default branch: a
maintenance push of the same SHA is distinct. Unrelated docs or release
workflows aren't implicit requirements. Consumer qualification admits each owned
push workflow independently.

The built-in v1 adapter supports original GitHub Actions runs under the
configured app authority, with complete applicable job/step success and
app-bound legacy protection contexts. Source must expose jobs and steps
directly; `continue-on-error` must be absent or literal `false`. Expressions
and reusable workflow calls need an adapter that can prove outcomes. The built-in adapter evaluates no expressions and rejects explicit job `if` and
unknown step conditions before publishing a candidate.
`simit::render::ci::step_conditions` defines each accepted step condition once:
the renderer uses its text, and the adapter uses the push outcome it must
observe on the original push.

| Condition                                                                 | On an original push                           |
| ------------------------------------------------------------------------- | --------------------------------------------- |
| `always()`, `!cancelled()`                                                | runs; needs no binding                        |
| `inputs.release`, `inputs.release && github.event_name != 'pull_request'` | skipped (inputs are absent)                   |
| default-branch publication, with or without the release alternative       | runs on the default branch, skipped elsewhere |
| `failure() && steps.simit-dependency-download.outcome == 'failure'`       | skipped                                       |

Bare conditions and those wrapped in `${{ }}` are equivalent. Other conditions
are allowed only on generated optional steps using pinned `actions/cache/save`.
It only saves a cache entry, so either outcome is accepted regardless of
condition. Grouped builds use it to save engine and input caches. Restore,
import, export and native KVM setup always run and decide inside their scripts.

A job with a skippable step must match every result to the exact candidate's
step ordinal and literal name. Missing, duplicate or unexpected outcomes fail
closed. Binding requires an exact job inventory: every job in that workflow
must prove its name, either literal or from a static include-only matrix. Rows
expand natively (`system` and `runner` rows without a `name`) or fill a `name`
template like `build (${{ matrix.group }})` from literal values. Every distinct
row must be present and successful. Skipped applicable gates never qualify;
the workflow and all jobs must succeed. These source checks apply to generator
and consumer qualification and pre-push admission of committed bytes.

Admission rejects required checks from other apps, unbound contexts, active
rulesets, locked or push-restricted targets, and PR-required protection for
direct landing. The API credential doesn't prove that the Git transport actor is
on a push allowlist. A separate REST endpoint reports signature-required
protection, which enters the policy digest. V1 rejects it before publication;
proving a forge-accepted signature on the candidate needs a supporting adapter.
Classic signature and pull-request requirements also reject matching uncreated
maintenance refs.

Admission merges repository, profile and project required checks. Consumer
requirements must match proved jobs in owned push workflows, normally literal
non-matrix job names (or IDs for unnamed jobs). For jobs gated by simit's
generated docs classifier, the GitHub adapter proves static matrix contexts
when the candidate diff against the base contains code (including `flake.lock`).
Docs-only changes and unknown classifiers fail; every gated job must still
succeed in the original run. Package-scoped workspace CI jobs named `MEMBER / JOB`
also satisfy `JOB`; every owned workflow's original run must succeed in full.
Hand-maintained-only checks and unproved dynamic/matrix contexts need a
supporting adapter or fail before receipt or maintenance ref publication.

Ruleset admission queries exact maintenance/adoption branches, including
uncreated maintenance refs, before receipt publication. Before either admission,
the adapter paginates all classic protections via the configured forge's
GraphQL API. It rejects any matching push/creation restriction, even wildcard
rules for uncreated refs. Bypasses and rule priority need an actor-aware
adapter. Missing permissions, partial GraphQL errors and incomplete rule
inventories fail admission. GitHub Enterprise Server uses `/api/graphql`
alongside its configured REST `/api/v3` root.

Complete workflow-registry pagination before admitting or qualifying a workflow.
Selected workflows must be active; disabled, incomplete or duplicate
observations fail.
A candidate may add a path absent from a complete registry but needs a genuine
original push run to qualify. For these new workflows only, admission verifies:

- Actions is enabled with all actions allowed (`actions/permissions`).
- Each job uses a hosted label (e.g. `ubuntu-24.04`, `macos-latest` or
  `windows-11-arm`) or labels carried together by one registered repository
  runner. `${{ matrix.KEY }}` resolves through static matrix values.
- The repository has every referenced `secrets.NAME` except `GITHUB_TOKEN`
  and every `vars.NAME`. Admission lists any missing names.

Concurrency must be absent or use a supported path/ref-scoped form below, with
absent or literal boolean `cancel-in-progress`:

- `${{ github.workflow_ref }}`
- `${{ github.workflow_ref }}-${{ github.ref }}`
- `${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}`

Admission also accepts generated prebuild groups with `github.workflow_ref`,
event, source repository, source branch and the `-prebuild` suffix. Their exact
cancellation expression is `!inputs.release && (push || pull_request)` or
`push || pull_request` for prebuilds without a release entry. Generated groups that coalesce push and pull-request runs must start with the
workflow's own path; admission requires `tags-ignore: ["**"]` on the push
trigger. This proves the concurrency condition for push qualification against
source without general expression evaluation (the original push run must still
qualify). Other expressions and job-level concurrency need a supporting adapter.
Display names don't prove uniqueness. A shared group can replace pending
originals even without cancellation. Selected workflow paths cannot collide
by case.

Protection contexts, CI engine variables and PR-only custody contracts don't
migrate; they need a later protocol with explicit ordered migration support.
Other forge/CI backends can enroll an absolute adapter command implementing the
same versioned inspect/qualify and source-binding contract. Requests carry
`ci`-generated paths in `workflows`; adapters choose workflow formats and
trigger semantics. The GitHub adapter reads owned Actions YAML at the exact
requested revision to identify push triggers.

Before receipt or maintenance ref publication, the adapter must support
`admit`. The request includes the exact generation `plan`, candidate revision,
branch and owned paths. Success returns one `admission` check under the enrolled
authority, bound to the revision, with `attempt = 1`, `status = "success"` and
`run` set to the SHA-256 of the complete request as recursively key-sorted
compact UTF-8 JSON. This prevents older adapters from treating admission as
qualification. Other operations omit the plan. Requests declare
`schemaVersion = 1` unless using a version-2 feature (currently `ciCredentialFile`).
V1 requests retain their exact bytes and admission digest. Adapters reject
versions inconsistent with request features and echo the version in responses.

The GitHub adapter checks filters against both qualification branches,
including ordered exclusions and tag-only triggers. It accepts literal branch
names and `*` / `**` globs; richer patterns fail closed. The built-in adapter
rejects workflows with push path filters before publishing the candidate unless
an adapter can evaluate GitHub's exact-change triggers. Initial admission
failure publishes no receipt or maintenance ref; retry after fixing policy or
templates. Failed admission on resume preserves the receipt and cannot push or
advance either ref.

### Forgejo + Crow adapter

`simit projects update-forgejo-adapter` qualifies Crow workflows for Forgejo
repositories. Set `apiUrl` to the Forgejo API root and `checkAuthority` to the
canonical Crow origin (no path or trailing slash). The adapter calls
`<checkAuthority>/api/v1`. Put the Forgejo token in `credentialFile` and the Crow
token in `ciCredentialFile`. Each token goes only to its own origin. Both files
must follow credential path, store and code-overlap rules and be distinct.
Dispatch also rejects identical snapshot bytes, including copies or links.

```nix
forges.codefloe = {
  host = "codefloe.com";
  apiUrl = "https://codefloe.com/api/v1";
  credentialFile = "/run/credentials/simit/forgejo-token";
  ciCredentialFile = "/run/credentials/simit/crow-token";
  checkAuthority = "https://ci.codefloe.com";
  adapter = {
    program = "${simit}/bin/simit";
    arguments = ["projects" "update-forgejo-adapter"];
  };
};
```

Requirements name workflows under the Crow origin, for example
`{name = "build"; authority = "https://ci.codefloe.com";}`. Use the literal
top-level `name:` or, if absent, the filename without its extension. That's
the name Crow reports and Forgejo uses in `ci/crow/push/<name>` contexts.

Inspection requires an unarchived, non-mirror repository and an active Crow
record with the Forgejo repository ID as `forge_remote_id`, plus the matching
clone URL and default branch. Remote IDs can repeat across forge connections.
The record must use `.crow/` and have no push approval gate
(`require_approval` of `none`, `forks` or
`pull_requests`). The token must be able to push the target branch.

Inspection checks every protection matching the target or maintenance branch.
Direct landing rejects approvals, signed commits, protected file patterns, and
disabled or allowlisted pushes. Required status contexts must be literal
`ci/crow/push/<name>`; globs, pull-request contexts and other CI systems fail.

Admission statically evaluates owned `.crow/*.yaml` and `.yml` files, rejecting
Jsonnet and keys outside the generated vocabulary. Workflows may use `name`,
`labels`, `platform`, `when`, `skip_clone`, `variables`, `workspace` and `steps`;
steps may use `name`, `image`, `commands`, `environment`, `directory`,
`entrypoint`, `failure: fail`, `temp_volumes` and `when`. Matrices, services,
dependencies, `runs_on`, approval gates, caches, `detach` and `failure: ignore`
fail before publication. Conditions allow only `event` and `branch` (literal
names, `*`/`**` globs or exactly `${CI_REPO_DEFAULT_BRANCH}`). A branch push
must run at least one owned workflow, and every required check must name one.
Default-branch-only workflows are carried unqualified by the maintenance
candidate and qualify after adoption, not on maintenance refs.

Qualification scans at most 20 pages of 50 pipelines, considering only pipelines
created at or after one hour before the revision's committer date. It needs
exactly one original push (`parent = 0`) for the exact revision and branch.
Restarts never qualify; duplicate originals fail. Crow's stored configuration
must match each owned workflow's forge bytes at that revision and cannot be
generated. The workflow and every step must succeed on the original attempt.
Qualification requires exactly the implicit `clone` step (unless `skip_clone`)
plus all and only the matching source steps: Crow drops nonmatching steps at
compile time. Each workflow reports a check with run
`<crow repository id>/<pipeline number>`. Exhausted CI quota (`CI quota denied ...`)
fails with a distinct message; retry after reset. GitHub Actions still qualifies
the generator; requests without workflows return `unsupported`.

## Pull-request landing

Set `landing` to `direct` (default) or `pull-request` per profile, with
per-coordinate overrides. Pull-request landing adds three operations in v2.
Requests carry `landing`, the `target` branch and the forge's `change` ID
once known. Responses echo `schemaVersion` and report `change` as
`{id, state, head, base, mergeCommit}` with `state` of `open`, `merged` or
`closed`. Requests without v2 features retain v1 bytes and admission digests.

- `propose` finds or creates the request with the maintenance branch at the
  exact candidate as head. Reuse open or merged requests, never closed unmerged ones.
- `merge` requires the exact candidate head, reports `pending` for approvals,
  checks, updates or forge mergeability computation, and fails on conflicts.
- `close` closes the request or reports it as merged if a human merged it first.

Inspection and target admission relax only approving-review protection: GitHub's
`required_pull_request_reviews` and classic `requiresApprovingReviews`. Push
restrictions, signatures, rulesets (including their merge queues), locks and
maintenance-branch protection still fail. GitHub chooses the first allowed
method: merge commit, squash, then rebase. The updater never self-approves;
required approvals wait for a human.

After candidate qualification and repeat target admission, the receipt enters
`proposed` and the updater proposes and merges. If merge is pending, the run
reports `proposed` and parks the repository behind unattempted entries in the
frozen rollout without failing the run or stopping the batch. A `proposed`
canary still gates the rollout.

On merge, the receipt records the merge commit as `landed` and enters
`adopting`. The target must equal `landed`, and `landed` must have the
candidate's tree and the candidate's source as its first parent. Then qualify
`landed` on the target. Merge, squash, rebase, and fast-forward are supported
subject to those exact checks; anything else fails closed and retains the
receipt for reconciliation.

Each run first re-reads a parked request:

- merged, including by a human: adopt as above;
- closed without merging: archive the receipt, delete the maintenance ref and
  regenerate;
- open but stale, because the target moved, the repository policy or default
  branch changed, or `projectPolicyDigest` changed: close it, archive, delete
  the ref and regenerate.

GitHub merges only while the target equals the candidate's source. A moved
target leaves the merge pending until the next run closes and regenerates it.
When the target requires linear history, skip merge commits in favor of the next
allowed method. Reading that rule needs `Administration: read`; without it the
updater assumes only that no linear-history rule applies. `--dry-run` reports
parked requests as `proposed` and never closes, proposes or merges requests or
deletes refs.

Deselecting or disabling a parked repository revokes the updater's authority
over that repository, so it no longer touches the open request or maintenance
ref. Clean them up by hand or re-enable the repository to resume or retire them.

Other forges add these operations in their own adapters.

## Pin work items

`simit projects update --pin-file PATH` applies an explicit
`fleetix.supply-work-items.v1` list from fleetix's `fleetix_supply_land`:

```json
{
  "contract": "fleetix.supply-work-items.v1",
  "producer": "github.com/caniko/harbor-rs",
  "target": "1111111111111111111111111111111111111111",
  "items": [
    {
      "coordinate": "github.com/caniko/foo",
      "relativePath": "owned/foo",
      "input": "harbor-rs",
      "target": "1111111111111111111111111111111111111111"
    }
  ]
}
```

Before state or inventory work, validation rejects unknown fields or contracts,
targets not exactly 40 hex digits, non-identifier inputs, and invalid or
duplicate coordinates. `--pin-file` narrows selection like `--repo`; the two
flags are mutually exclusive. Ineligible items report `skipped` with a reason
(`missing`, `unknown-forge`, `disabled` or a selection exclusion like
`coordinate-enrollment`). Selection fails only if no item is eligible. Canaries
are optional in the list but still run first when present.

The updater finds the repository's single `<input>.url`, `inputs.<input>.url`
or `<input> = { url = "…"; … };` declaration and rewrites its revision:
`rev=` for `git+…` URLs, or the path segment for `github:owner/repo`. It runs
`nix flake lock` with a private home and cache in repository state, then compares
the old and new locks. `flake.nix` and `flake.lock` join as patches under the
usual commit, admission, landing and baseline rules. Any other changed path
fails the item. Defer README badges until the next run: they use the pre-pin
checkout. If the `flake` scope also touches `flake.nix`, fail rather than
claiming it twice.

The commit subject is `chore(deps): pin <input> to <target[..12]>`. If the CI
plan changes files too, the second body line records the adopted generator
revision. Unrewritable items commit nothing and report `skipped`:
`unrecognised-input-declaration` for missing, ambiguous or unsupported
declarations or URL forms; `branch-ref-github-url` for `github:owner/repo/<branch>`.
Frozen rollouts store the items. Repeat `--push` with the same file to resume;
a different file won't resume the rollout or reuse a pending candidate because
the pin-scoped policy digest won't match. Profiles accepting pin work items need
`nix` in `runtimePackages`.

Before rendering, the updater aligns an enrolled consumer's literal flake
input for the generator source coordinate and its `flake.lock` to the enrolled
revision. For consumers with that matching literal input, the updater also
renders the full CI plan and any existing simit-generated release workflow so
the drift-check generator sees the same artifacts.
Matching URLs with unsupported pin syntax fail. Flake edits stay patches, and
new managed artifacts still require enrollment. Only primary CI qualifies the
candidate; including a release workflow doesn't run a release.

## RustSec lockfile repairs

The generator has no network, an empty private home and a read-only checkout,
so it cannot fetch the RustSec database or a registry index even with `cargo` and
`cargo-audit` in `runtimePackages`. An offline audit could use an explicitly
provisioned database snapshot, but offline `cargo update` can miss fixes
absent from the local index.

Repair in a separate lockfile-only PR. Use the project's normal Rust toolchain
to run `cargo update -p NAME@OLD_VERSION --precise FIXED_VERSION`, review the
`Cargo.lock` diff, then rerun `cargo audit` and locked CI. If no compatible fix
exists, defer to a maintainer. Don't suppress advisories to qualify fleet candidates.

An opt-in updater would need a separate credential-free resolver with a
pinned advisory database and approved registry metadata, writing only to a
disposable copy. Propose a `Cargo.lock` patch only after proving compatibility,
an in-scope diff and a second audit clear of the advisory. Keep the
exact-candidate CI gate and never give the generator network access.

## Scopes

`scopes` lists the generator scopes a profile maintains. Each scope updates an
existing consumption; none introduces a new one. Include `ci` and list each
scope at most once.

| Scope      | Generator call                       | Result                                                                                                                                                                                                                                             |
| ---------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ci`       | `init ci --primary-only --plan-json` | Primary CI. Its files are the only qualification set.                                                                                                                                                                                              |
| `ci-extra` | `init ci --plan-json`                | Also publishers, Pages, extension publishers, release-plz, review, custody and `scripts/ci-local.sh`. The persisted `[ci]` table and a newly created `deny.toml` are patches.                                                                      |
| `release`  | `init release --plan-json`           | The release workflow, only when a generated one exists. It moves to another backend only when `[ci].platform` or a migration selects one.                                                                                                          |
| `flake`    | `init flake --plan-json`             | Flake and hook files, only when simit already manages them. A `flake.nix` change whose input names differ from `flake.lock`'s root inputs is rejected, as is a changed declaration of an existing input, since generation cannot fetch a new lock. |
| `readme`   | `upgrade --plan-json`                | A `README.md` badge-block patch, only when the badge markers exist.                                                                                                                                                                                |

`ci` must match its part of `ci-extra` byte for byte, including obsolete paths.
Scopes cannot overlap paths; patches cannot target `.github/workflows`,
`.forgejo/workflows` or `.crow`. `workflows.json` and qualification requests
carry only `ci` files. Other scopes commit and pass admission with the candidate
but first run after landing, without their own qualification. The full `init ci`
plan never resolves the local release trust root; a plan containing publishing
workflows instead requires the committed trust root those workflows fetch. With
flake `simitConfig`, the full plan can render existing policy but cannot persist
changes to the flake's `[ci]` table or select a new backend.

A patch edits a whole project-owned file. Plans with `patches` use
`simit.ci-update-plan.v2`; plans without them use `simit.ci-update-plan.v1`.
The updater commits patches, records Git mode and blob in receipt `patches`,
verifies them on resume and includes them in admission. Plans with patches
use v2 admission and receipts. Baselines never own patches, so later README
or `simit.toml` edits don't block updates.

Widening `scopes` usually claims existing files. As with initial adoption,
adding them to an existing baseline requires `enrollCurrent`. Baselines record
the adopted scopes, so only the run adding a scope can claim them. Later runs
again stop if a plan claims an existing file outside the baseline.

Without scope widening, a profile with `enrollCurrent` adopts a planned workflow
whose committed copy carries the ownership marker (`# Generated by simit. Manual
edits will be reported as ci=drift.`). It marks earlier generator output, often
from enabling and regenerating `[prebuild]` by hand after baseline adoption. A
committed file without the marker remains project-owned and fails adoption.

## Provider migration

`migrate = { provider; platform; runner; }` regenerates CI for `actions` on
`github` or `crow` on `forgejo`. Forgejo Actions is not a migration target
because the Forgejo updater adapter qualifies only Crow pipelines.

`init ci` gets `--ci-provider`, `--platform`, `--runner` and
`--reset-step-runners`. The reset clears old `[ci].step_runners` labels and
Nix-only `[ci].nix_system_runners`. `init release` gets the provider, platform
and runner, replacing the `[ci].runner` fallback for release jobs. The rewritten
`[ci]` table is a `ci-extra` patch, so migration requires `ci-extra` and forbids
`readme` (badges use pre-migration workflows). The obsolete sweep retires old
generator-owned workflows like `.crow/*`. Without an adopted baseline, set
`enrollCurrent` for the first migrated run.

```nix
profiles.migrate-provider = {
  scopes = ["ci" "ci-extra"];
  migrate = {
    provider = "actions";
    platform = "github";
    runner = "ubuntu-24.04";
  };
  enrollCurrent = true;
  # workspace, generator, select and requiredChecks as for any profile
};
```

## Dependencies and verification

The updater reuses Rust dependencies and flake inputs. In `runtimePackages`,
add OpenSSH for SSH transport, Cargo for Rust metadata, Nix for flake config
and the configured signing program as needed.

Use the lightweight native test shell to avoid documentation/site provisioning:

```sh
nix develop .#test -c cargo test --locked --lib projects_update
nix develop .#test -c cargo test --locked --test init_ci \
  primary_plan_is_versioned_read_only_and_preserves_other_owners
```

Reuse a dedicated `--target-dir`. A fresh one rebuilds existing dependencies;
that doesn't signal new dependencies.

CI's `nix flake check` evaluates `checks.<system>.simit-updater-config` to check
typed policy rendering and NixOS service/timer configuration with custom
workspace paths, runtime tools and a credential file absent during evaluation.

Bubblewrap runtime tests can't run in Nix's build sandbox. Package and nextest
derivations exclude only `namespace_*` engine tests; other engine checks stay in
package qualification. Hosted native `cargo test --all-features` and
`cargo test --no-default-features` run the full group, including credential
isolation and locked offline inputs. Hosted CI runs the updater group separately
in both feature modes before the full Nix and runtime gates so neither focused
result is hidden. Packaged-generator qualification separately runs store-wrapper,
digest-enrolled wrapper, no-input and locked-input cases, retaining nested
diagnostics for each failure. These are previews. Connected API replay tests
built-in adapter inspection, both branch admissions and original qualification.
Live GitHub publication still needs separate controlled-repository evidence.
