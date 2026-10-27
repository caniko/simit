# Workstation CI updates

The dev-station flake declares which repositories may receive generated CI
updates, the qualified generator revision, forge connections and rollout limits.
Simit executes that policy using disposable clones and persistent receipts.

## NixOS configuration

Import `inputs.simit.nixosModules.updater`. The executor package defaults to that
input's package; each profile independently enrolls its generator executable and
source revision. `inputs.simit.lib.mkUpdateConfig` renders the same versioned
policy for standalone installations.

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

All identities, paths, endpoints, authorities and checks in this example are
operator policy. Use the actual qualified generator source and required check
names for the selected repositories. A non-store generator must additionally
enroll `executableSha256`. Executable scripts require an immutable Nix-store
interpreter chain; `/usr/bin/env` and mutable system interpreters are rejected.
Native executables must be store-backed even with an enrolled digest: a digest
alone does not bind their dynamic loaders or shared libraries.

Runtime roots and credential paths are strings. Nix evaluation does not inspect
worktrees, query forges or read credential contents. Git transport authentication
is supplied by the execution user's existing Git configuration; `credentialFile`
authenticates API reads only, except under pull-request landing, where it also
opens, merges and closes pull requests. A fine-grained GitHub token then needs
`Pull requests: write` and `Contents: write` on the enrolled repositories. The execution user's Git identity, signing and
hooks are honored when creating candidate commits.
The sandbox has no personal Simit configuration. Projects must declare their CI
platform and any private runner labels through project policy; GitHub can use
its built-in hosted-runner defaults. An undeclared platform retains Simit's
Forgejo default and cannot borrow runner configuration from the operator's home.

The module creates `simit-updater-<profile>.service` and, when scheduled, its
timer. State is private and writable; workspace source and user configuration
remain read-only in the service. The selected user must already exist unless
using the module's dedicated `simit-updater` user. Services load the generated
regular Nix-store policy file directly; `/etc/simit/updates.json` remains its
operator-facing link. Provision that user's Git
identity, signing configuration, transport and credential-file access.
Each service can write only the state directory of its profile's workspace.
Profiles sharing a workspace wait on its exclusive updater lock before discovery
or transaction work; the acquired lock's file identity is verified after waiting.
Workspace state directories must be disjoint, including after runtime symlink
resolution. Enrollment digests use exactly 64 lowercase hexadecimal characters.
Generator source revisions require full 40-character lowercase commit SHAs.
Inventory paths, command executables and credential files are absolute runtime
paths. Rust and Nix reject store credential paths and credential/code overlaps
without reading the filesystem. At runtime credentials must also resolve outside
the store: reads verify the opened regular file's identity through Linux procfs
and snapshot bytes from that same descriptor.

## Selection and inventory

Supply exactly one `inventory.file` or `inventory.command`. Commands are absolute
executables with argv arrays and timeouts of 1 through 900 seconds. Inventory
freshness is bounded to 1 through 86400 seconds. Their JSON snapshot has:

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

Every profile requires a non-empty `select.coordinates` list declared by the
workstation operator. Inventory observations cannot add to that enrollment;
their `policyDigest` records provenance, not deployment authority. Exact
coordinates are rechecked when processing frozen selections as well.
Configured canaries must be a subset of that coordinate enrollment; both Nix
and runtime validation reject an unlisted canary before discovery.
Each forge hostname must have exactly one configured policy. Inventory can name
that policy but cannot choose a weaker adapter, credential or check authority for
the same host. Rust and Nix reject duplicate host entries before discovery.
Forge hosts and the hostname portion of every enrolled or discovered coordinate
must use lowercase. Validation rejects case aliases before discovery; it does
not silently change enrollment or receipt keys. Remote DNS host comparison is
case-insensitive, while repository owner/path spelling remains exact.

Inventory commands run in read-only Linux filesystem, network and PID namespaces,
with a private writable temporary directory. The host `/run` socket tree and Nix
daemon socket are hidden, and descendants cannot outlive discovery. The same persistent-state
checkpoint used for generation also surrounds discovery, with no state subtree
excluded. Unexpected writes are detected and restored before inventory output can
reach a repository transaction, including on failure.
The operator-facing policy path may be a symlink, as with NixOS's
`/etc/simit/updates.json`; its resolved target must pass the regular-file JSON
reader. State and evidence readers retain their strict regular-file checks.

The timestamp must be fresh at rollout start. Metadata selectors are combined
with AND, path exclusions win, and exact coordinates disambiguate short names.
Relative path globs use component boundaries: `*` does not cross `/`, while
`**` may. The Nix renderer admits literal components, `?`, `*` and complete `**`
components; malformed recursive globs and bracket classes are rejected before
policy generation. Worktrees and mirrors are observations, not separate default targets.
Duplicate selected canonical identities are errors. `custom` and `excluded` CI
policies are excluded. Forge connections and the profile's eligible selection
jointly determine deployment authority; inventory alone does not enroll a repo.

An inventory adapter can reuse an existing workspace catalog. Canix's
`workspace simit-inventory --raw --forge HOST=KEY` projection uses its configured tree,
sources, exclusions and canonical/mirror policy. Its lifecycle is `observed`;
select that explicitly when enrolling these observations, or supply an inventory
producer with independently declared lifecycle data.

## Commands and transaction

```sh
simit projects update --profile fleet --plan --json
simit projects update --profile fleet --dry-run
simit projects update --profile fleet --push
simit projects update --profile fleet --status
```

`--plan` explains every operator-enrolled coordinate, including missing inventory
entries, disabled overrides, CLI narrowing and observations rejected by kind,
lifecycle, access, checkout role, CI policy or path selectors. Its versioned JSON
report retains inventory and policy digests and explicitly marks qualification
`not-run`. It performs protected inventory discovery under the workspace lock,
without invoking forge adapters, reading forge credentials, verifying a generator,
cloning repositories or creating candidates or rollout progress. Empty selections
are reportable, including a configured canary rejected by lifecycle or other
selection filters. Execution still requires every canary to be eligible and
included in CLI narrowing. Duplicate eligible canonical identities, unlisted
canaries and invalid CLI selection remain errors. `--plan` cannot be combined with execution/status modes
or a batch limit; it reports the full selection before batching.
Disabled enrollment entries carry the override's `reason` and, when declared,
its `blocker`: a kebab-case code (`[a-z][a-z0-9-]*`) that fleet tooling can
group by. Rust and Nix reject a `blocker` on an enabled override.

Use `--config PATH` outside NixOS. `--repo HOST/OWNER/REPO` narrows selection;
configured canaries must remain included. `--limit N` further bounds the profile's
batch size. Repeating `--push` resumes the recorded rollout and candidate commit.
`batchSize` and `maxConcurrent` are bounded to `1..4294967295` in Nix, matching
the runtime's unsigned 32-bit representation.

The executor freezes the selected identities, then processes canaries before
ordinary repositories. A failed or incomplete canary stops that batch even if
`stopOnFailure = false`. Execution is serial, staying below `maxConcurrent`; larger
values are ceilings, not a promise of parallelism. Each invocation processes at
most `batchSize` repositories, and subsequent invocations continue the snapshot.

For each repository:

1. Resolve forge identity, remote default branch, target revision and protection.
2. Qualify the pinned generator source and verify its enrolled executable.
3. Obtain `init ci --primary-only --plan-json` in an isolated Linux sandbox. The versioned
   ownership plan supplies writes and deletions; it uses the project's existing
   resolved configuration without creating a new configuration file. Generation
   must preserve persistent updater state, Git configuration, hooks and refs as
   well as source files. State outside the current checkout is checkpointed and
   verified before trusting the plan, including when generation fails. Detected
   mutations are restored and the transaction fails.
4. Compare managed files with the adopted baseline, prepare a scoped candidate
   commit, and retain its diff and identity. Every present managed path must stay
   in the plan or be explicitly listed as obsolete; omission cannot drop ownership.
5. Admit the exact generated plan for both maintenance and adoption branches,
   including when resuming a retained candidate, then push a maintenance ref and
   require exact-candidate original push qualification.
6. Reinspect source and policy, then conditionally advance the target branch to
   the same candidate using an explicit Git lease after repeating target admission.
   With `landing = "pull-request"` the updater instead proposes and merges a pull
   request (see below).
7. Verify exact-source default/target-branch adoption, record a new baseline and
   delete the maintenance ref under a lease on the candidate.

Managed identities include the canonical Git mode and blob from the staged/committed
tree, so clean/smudge filters and newline conversion do not invalidate the next
rollout. Executable-bit changes require reconciliation even when the content is
unchanged. Working-copy checks honor Git's `core.fileMode` setting. Older blob-only
baselines lack mode evidence and require explicit reconciliation before reuse.
An adopted receipt with a missing baseline requires explicit reconciliation;
`enrollCurrent` cannot silently initialize it again. Serialized state must fit the
same size limit used by the reader before it can replace a retained receipt.

Generation exposes only the read-only candidate checkout, the enrolled executable,
and the Nix store through a disposable copy-on-write overlay. The child has a cleared environment, private temporary
home, network and PID namespaces, and no updater state, credential files, SSH agent,
host Nix daemon or host fetcher cache. Its PID-1 lifetime kills and reaps descendants
before successful generation completes. The native `sandboxPackage` is configurable
in the dev-station flake; standalone users supply immutable store-backed `bwrap`
and runtime tools on `PATH`. Missing namespace support fails closed.
Hosts restricting user namespaces through AppArmor must provision an allowance
for the enrolled immutable Bubblewrap executable through their host configuration.
The updater does not change host policy. Hosted qualification records the initial
probe and kernel diagnostics before loading an executable-scoped allowance on
its disposable runner, then repeats the probe with isolation still enabled.

Flake configuration forbids input/lock updates, flake-provided Nix settings and
import-from-derivation. Evaluation uses a disposable local store without network
access, initialized from a consistent SQLite snapshot of the host's Nix store
registrations and its schema marker. The snapshot includes committed WAL contents; the live database
and daemon are never exposed to generation. `NIX_STATE_DIR` may select the host
state directory; its default is `/nix/var/nix`. Locked inputs already registered
in the host store remain available offline. Missing inputs require explicit
preparation rather than credentialed fetching during generation. Updater execution
currently requires Linux. SQLite is supplied by the NixOS module; standalone
users include an immutable `sqlite3` on `PATH` for flake-backed configuration.

Existing workflow branch filters must admit `maintenanceRefPrefix`. Generated
Crow `build` and `ci` workflows admit only the default prefix
(`simit/update/**`) beside `${CI_REPO_DEFAULT_BRANCH}` for push pipelines; a
profile with another prefix gets no push pipelines on its maintenance refs. Missing
candidate events, checks or capability support cannot authorize landing. Remote
movement, changed default branches, protection changes and managed-file edits
invalidate earlier candidate acceptance. Failed, skipped, cancelled, neutral or
rerun required cases cannot qualify. Receipts and adapter responses remain in
`stateDirectory`; `--status` reads them without discovery or forge queries.
Each receipt records the `generatorRevision` that produced it; receipts written
before that field existed report `null` until their next transaction.

Repository clones, receipts and baselines are isolated under a profile-specific
directory in `stateDirectory`. `--status` reports only the requested profile.
Receipt policy hashes include the active profile, its workspace, and its generator
and consumer forge records. Receipts written by this version (`schemaVersion: 2`)
also record `projectPolicyDigest`, the same hash with other coordinates'
enrollment, canary membership and overrides removed; unfinished v2 receipts
resume against it, so editing another repository's policy keeps this one's
candidate or pull request. Receipts without `schemaVersion` are v1, resume
against `configDigest` and land directly. Older executors reject v2 receipts. Frozen selection keys use the profile, workspace and
generator forge policy. Changes to unrelated configuration entries therefore
preserve the frozen selection and resumable candidates; changes to a transaction's
referenced policy still invalidate its pending receipt. Frozen projects revalidate
against the currently enrolled consumer forge host before any adapter request.

Before landing, source movement archives the invalidated receipt, deletes its
maintenance ref and prepares a fresh candidate from the newly observed source,
preserving the adopted baseline. Ref deletion is cleanup: a ref that is already
gone, or that another writer moved, is logged and kept rather than failing.
A candidate already awaiting adoption, or one found in the new source's ancestry,
requires explicit adoption reconciliation instead of silently restarting.

Interrupted generation, inventory discovery or an unsuccessful checkpoint restoration leaves
`generation-checkpoint.json` in the workspace state root, naming the retained
recovery snapshot in that same durable state directory. Recovery bytes are synced
before the marker is published; service-private temporary storage is used only
for disposable tool output. Further transactions stop before adapter dispatch.
Stop the workspace's updater services and reconcile state against that snapshot
before removing the marker. The snapshot excludes the current disposable checkout
and preserves the other state files and their permissions.

The updater also verifies the workspace lock's file identity across generation
and before saving rollout progress. Replacing the lock stops the transaction and
retains recovery evidence rather than restoring state under a different lock.

Provision each service state directory with mode `0700`, owned by its configured
user. The module does not create arbitrary state paths through root-owned tmpfiles.
At runtime the updater resolves existing path components, checks every state tree
against the resolved workspace roots, and rejects overlaps before creating files.

Forge adapters execute in Linux namespaces with an empty filesystem view populated
by the read-only Nix store, enrolled code snapshots, the exact request, the
configured credentials, DNS plumbing and immutable CA certificates. They retain
network access to their forge. Home and temporary storage are private, host state
and daemon sockets are absent, and descendants cannot outlive the adapter.

Forge adapters require a store-backed executable or `adapterSha256`. File-valued
configured arguments must be absolute store paths or enrolled in `adapterInputs`
as path-to-SHA-256 entries. Digest-enrolled mutable files are executed or read from
fresh private snapshots for each verification, retaining prior verified bytes
even when the same input is re-enrolled or supplied more than once. Store inputs
must resolve to files inside the store. Adapters
and generators retain their enrolled store installation paths even when a digest
is supplied, preserving wrapper resources and installation-relative loaders.
Containment verification resolves symlinks without replacing the executable's
invocation name, which can select a multicall applet.
Every `adapterInputs` entry is checked, including helpers not named on argv.
The adapter executable must not also be listed in `adapterInputs`; enroll its
digest through `adapterSha256`. Rust and Nix reject duplicate entrypoint paths
before dispatch, including equivalent paths with repeated separators.
The credential path must also differ from the executable and every declared
input; Rust and Nix reject those overlaps before dispatch.
Verified mutable inputs are mounted at their enrolled absolute paths so scripts
can import declared siblings; other mutable helpers are inaccessible. These
checks also apply to store interpreters executing a configured script argument.
Mutable host originals are never mounted into the adapter; only their verified
snapshots are exposed. Adapters run from a private directory, with runtime values
supplied through request JSON.
Script interpreter chains must also resolve entirely inside the store. Shebang
arguments are restricted to no arguments, Python's isolated `-I`, or
shell `-e`; arguments that can select another script or loader are rejected.
Tool execution clears runtime-injection environment variables and disables Python
user-site imports; Python scripts should use an isolated (`-I`) store interpreter.
Inventory launchers start with a cleared environment and an explicit runtime
allowlist. Adapters use only store-backed `PATH`/CA paths and private home/temp
settings. Forge/package-registry credentials and unlisted variables are excluded;
adapters receive their configured `credentialFile` and `ciCredentialFile`
through request JSON and read-only file mounts. The updater's separate Git
transport commands retain operator Git identity, signing and transport configuration.
Repository-routing variables such as `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`,
object directories and ref namespaces are removed from every updater Git command
so an inherited environment cannot redirect operations outside the disposable clone.
HTTPS clone URLs must not contain a username or password; SSH usernames remain
supported through operator Git authentication. Clone URLs must use the protocol's
default port, because enrollment identifies the service by hostname. Override
target branches are validated as complete Git branch refs before discovery; the
Nix policy renderer enforces the same branch grammar.

Generator qualification selects push workflows containing the configured required
check names under their enrolled application authority on the generator repository's
current default branch. The API query and admitted original runs both bind that
branch, so a maintenance push of the same source SHA is a distinct observation.
Unrelated docs or release
workflows do not become implicit generator requirements. Consumer qualification
continues to admit every owned push workflow independently.

The v1 built-in adapter supports GitHub Actions originals with configured app
authority, complete applicable job/step success and app-bound legacy protection contexts.
Workflow source must expose its jobs and steps directly: `continue-on-error` is
accepted only when absent or the literal `false`. Expressions and reusable
workflow calls require an adapter that can establish the underlying outcomes.
Explicit job `if` conditions and unknown step conditions require a supporting
adapter and are rejected before publishing a candidate. A single source job may
use the generated prebuild's exact branch-push predicates: `inputs.release`, its
non-PR variant, default-branch publication (or release), and `always()`. On an
original push, release inputs are absent: release-only steps must be skipped,
maintenance publication must be skipped, and default-branch publication and
cleanup must succeed. Every conditional result must match the exact candidate
workflow's step ordinal and literal name. Missing, duplicate or unexpected
outcomes fail closed. Conditional native matrix jobs support only a static,
include-only system/runner matrix with native job names; every distinct source
row must be present and successful. A skipped applicable gate never establishes
acceptance, and the workflow and all jobs must still succeed.
This source check applies to generator and consumer qualification, and to
pre-push admission of committed candidate bytes.
Required checks from other applications, unbound contexts, active rulesets,
locked or push-restricted target branches and, for direct landing, PR-required
protections are rejected during admission. The API credential is not evidence of the separate Git
transport actor's membership in a push allowlist.
Signature-required protection is queried through its separate REST endpoint and
included in the observed policy digest. V1 rejects it before publication; proving
a forge-accepted candidate signature requires a supporting adapter. Classic
signature and pull-request requirements also reject matching uncreated maintenance refs.
Admission carries the merged repository, profile and project required checks.
Each consumer requirement must match a literal non-matrix job name (or job ID
when unnamed) in an owned push workflow. Requirements produced only by
hand-maintained workflows or dynamic/matrix contexts require a supporting
adapter and are rejected before publishing a receipt or maintenance ref.
Ruleset admission queries each exact maintenance/adoption branch, including a
maintenance ref that has not been created, before publishing a prepared receipt.
Classic protection rules are also paginated through the configured forge's
GraphQL API before either admission. Any matching push/creation restriction is
rejected, including wildcard rules for uncreated maintenance refs. Rule bypasses
and priority require an actor-aware adapter. Missing permissions, partial GraphQL
errors and incomplete rule inventories fail admission. GitHub Enterprise Server
uses `/api/graphql` alongside its configured REST `/api/v3` root.
The workflow registry is completely paginated before admission and qualification.
Existing selected workflows must be active; disabled, incomplete or duplicate
observations fail. A path absent from a complete registry may be introduced by
the candidate, but still needs a genuine original push run to qualify.
Concurrency is admitted only when absent or in the supported path/ref-scoped
forms below, with an absent or literal boolean `cancel-in-progress`:

- `${{ github.workflow_ref }}`
- `${{ github.workflow_ref }}-${{ github.ref }}`
- `${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}`

The generated prebuild form using `github.workflow_ref`, event, source repository,
source branch and the `-prebuild` suffix is also admitted, including its exact
`!inputs.release && (push || pull_request)` cancellation expression. This is a
source-bound proof for push qualification; no general expression evaluation is used.
Other expressions and job-level concurrency require a supporting adapter.
Workflow display names do not establish uniqueness. Even with cancellation
disabled, a shared group can replace pending originals. Case-colliding selected
workflow paths are rejected as well.
It does not migrate protection contexts, CI engine variables, or PR-only custody contracts. Those
transitions require a later protocol with explicit ordered migration support.
Other forge/CI backends can enroll an absolute adapter command implementing the
same versioned inspect/qualify contract and its source-binding requirements.
The request's `workflows` list carries all owned generated paths. Each adapter
selects its own workflow formats and trigger semantics. The GitHub adapter reads
owned Actions YAML at the exact requested revision to identify push triggers.
Before publishing a candidate receipt or maintenance ref, the adapter must support
the `admit` operation. Its request includes the exact generation `plan`, candidate
revision, branch and owned paths. Success includes one `admission` check under
the enrolled authority, bound to that revision and to the SHA-256 of the complete
request's recursively key-sorted compact UTF-8 JSON in `run`, with `attempt = 1`
and `status = "success"`. This prevents older adapters from silently treating
admission as qualification. The plan field is omitted from other operations.
Requests declare `schemaVersion = 1` unless they use a version-2 feature
(currently `ciCredentialFile`); version-1 requests keep their exact bytes and
admission digest. Adapters reject a declared version that differs from the
request's features, and responses echo the request's version.

The GitHub adapter checks branch trigger filters against both qualification
branches, including ordered exclusions and tag-only triggers. Literal branch
names and `*` / `**` globs are supported; richer pattern syntax fails closed.
Path-filtered push workflows are rejected before publication; supporting them
requires an adapter that can evaluate GitHub's exact-change trigger semantics.
Initial admission failures leave no published candidate receipt or maintenance
ref and can be retried after correcting policy or templates. A failed resume
admission preserves the prior receipt and cannot push or advance either ref.

### Forgejo + Crow adapter

`simit projects update-forgejo-adapter` qualifies Crow workflows of repositories
hosted on Forgejo. `apiUrl` is the Forgejo API root and `checkAuthority` is the
canonical Crow origin (no path or trailing slash); the adapter calls Crow at
`<checkAuthority>/api/v1`. `credentialFile` holds the Forgejo token and
`ciCredentialFile` the Crow token. Each token is sent only to its own origin;
both follow the credential path, store and code-overlap rules and must be
distinct files. At dispatch the snapshotted tokens must also differ, so links or
copies of one secret are refused.

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

Requirements name Crow workflows under the Crow origin, for example
`{name = "build"; authority = "https://ci.codefloe.com";}`. A workflow's name is
its literal top-level `name:`, otherwise its file name without extension, as
Crow reports it and as Forgejo shows it in `ci/crow/push/<name>` status contexts.

Inspection requires an unarchived, non-mirror repository whose active Crow record
names the Forgejo repository ID as `forge_remote_id` and its clone URL (remote IDs
repeat across forge connections), shares its default branch, uses the default
`.crow/` configuration path and has no push approval gate (`require_approval` of `none`,
`forks` or `pull_requests`). The token must be able to push the target branch.
Every protection rule matching the target or maintenance branch is checked:
approvals, signed commits, protected file patterns, and disabled or allowlisted
pushes are rejected for direct landing. Required status contexts must be literal
`ci/crow/push/<name>`; globs, pull-request contexts and other CI systems fail.

Admission statically evaluates owned `.crow/*.yaml` and `.yml` files. Jsonnet and
keys outside the generated vocabulary are rejected: workflows may use `name`,
`labels`, `platform`, `when`, `skip_clone`, `variables`, `workspace` and `steps`;
steps may use `name`, `image`, `commands`, `environment`, `directory`,
`entrypoint`, `failure: fail`, `temp_volumes` and `when`. Matrices, services,
dependencies, `runs_on`, approval gates, caches, `detach` and `failure: ignore`
therefore fail before publication. Conditions may use only `event` and `branch`
(literal names, `*`/`**` globs, or exactly `${CI_REPO_DEFAULT_BRANCH}`). A push
to the branch must run at least one owned workflow, and each required check must
name one. Workflows that run only on the default branch are carried unqualified
by the maintenance branch and qualified after adoption.

Qualification finds the single original push pipeline (`parent = 0`) for the
exact revision and branch among pipelines created from one hour before the
revision's committer date, scanning at most 20 pages of 50. Restarts never
qualify; duplicate originals fail. Crow's stored configuration for each owned
workflow must equal the forge's bytes at the revision and must not be generated.
The workflow and every step must be original attempts that succeeded, and the
steps must be exactly the implicit `clone` step (unless `skip_clone`) plus the
source steps whose conditions match the push, because Crow drops the rest when it
compiles the workflow. Each workflow becomes a check with run
`<crow repository id>/<pipeline number>`. A workflow Crow refused for an
exhausted CI quota (`CI quota denied ...`) fails with a distinct quota message;
rerun the update after the quota resets. Generator qualification stays with
GitHub Actions; requests without workflows answer `unsupported`.
## Pull-request landing

`landing` is `direct` (the default) or `pull-request`, set per profile and
overridden per coordinate. Pull-request landing uses adapter protocol v2: the
request carries `landing`, the `target` branch and, once known, the forge's
`change` identifier, and adds three operations. Responses echo the request's
`schemaVersion` and report `change` as `{id, state, head, base, mergeCommit}`,
where `state` is `open`, `merged` or `closed`. Requests without these features
stay version 1 with unchanged bytes and admission digests.

- `propose` returns the open or merged pull request whose head is the
  maintenance branch at the exact candidate, creating one when none exists.
  Unmerged closed requests are history and are not reused.
- `merge` merges only when the head is still the candidate. It reports
  `pending` while the forge needs approvals, checks, an update or its own
  mergeability computation, and fails on conflicts.
- `close` closes the request, or reports it as merged when a human already
  merged it.

Inspection and target-branch admission relax only approving-review protection:
GitHub's `required_pull_request_reviews` and the classic
`requiresApprovingReviews` rule. Push restrictions, signatures, rulesets (and so
ruleset merge queues), locks and maintenance-branch protection stay rejected. GitHub
uses the first merge method the repository allows: merge commit, then squash,
then rebase. The updater never approves its own request; repositories needing
approvals wait for a human.

After the candidate qualifies and the target is admitted again, the receipt
enters `proposed` and the updater proposes and merges. If the request cannot
merge yet, the run reports `proposed` and the repository is parked: it does
not fail the run or stop a batch, stays in the frozen rollout and moves behind
unattempted repositories. A canary in `proposed` remains a barrier.

A merged request records `landed` (its merge commit) and enters `adopting`.
Adoption requires the target to be exactly `landed`, `landed` to have the
candidate's tree, and its first parent to be the candidate's source; then
`landed` is qualified on the target. A merge, squash, rebase or fast-forward
all satisfy this; anything else fails closed and keeps the receipt for
reconciliation.

Each run first re-reads a parked request:

- merged, including by a human: adopt as above;
- closed without merging: archive the receipt, delete the maintenance ref and
  regenerate;
- open but stale, because the target moved, the repository policy or default
  branch changed, or `projectPolicyDigest` changed: close it, archive, delete
  the ref and regenerate.

GitHub merges only while the target is still the candidate's source; a moved
target leaves the merge pending until the next run closes and regenerates it.
When the target requires linear history, merge commits are skipped in favour of
the next allowed method; reading that rule needs `Administration: read`, and
without it the updater assumes no such rule. Previews (`--dry-run`) report a parked request as
`proposed` and never close, propose or merge, or delete a ref.

Deselecting or disabling a parked repository revokes the updater's authority
over it, so the updater no longer touches its open request or maintenance ref;
close those by hand, or re-enable the repository so a run resumes or retires them.

Other forges add these operations in their own adapters.

## Dependencies and verification

The updater reuses existing Rust dependencies and flake inputs.
`runtimePackages` controls native service tools;
add OpenSSH for SSH transports, Cargo for Rust metadata, Nix for flake config,
and the configured signing program when needed.

Use the lightweight native test shell to avoid documentation/site provisioning:

```sh
nix develop .#test -c cargo test --locked --lib projects_update
nix develop .#test -c cargo test --locked --test init_ci \
  primary_plan_is_versioned_read_only_and_preserves_other_owners
```

Reuse a dedicated `--target-dir` across invocations. A fresh directory compiles
existing dependencies and is not evidence of dependency additions.

The existing CI `nix flake check` also evaluates `checks.<system>.simit-updater-config`.
It checks typed policy rendering and the NixOS service/timer configuration with
custom workspace paths, runtime tools and a credential file that does not exist
during evaluation.

Bubblewrap's runtime tests cannot run inside Nix's build sandbox. Package and
nextest derivations exclude only engine tests named `namespace_*`; the other
engine checks remain in package qualification. The hosted native
`cargo test --all-features` and `cargo test --no-default-features` steps run the
complete group, including credential isolation and locked offline inputs.
Hosted CI runs the updater group independently in both feature modes before the
full Nix and runtime gates, so a broad check cannot hide either focused result.
The packaged-generator qualification runs store-wrapper, digest-enrolled wrapper,
no-input and locked-input cases independently, retaining nested tool diagnostics
for every failure. These are preview checks. The built-in adapter's connected
API replay checks inspection, both branch admissions and original qualification;
live GitHub publication still requires separate controlled-repository evidence.
