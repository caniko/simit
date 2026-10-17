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
    runtimePackages = with pkgs; [git cargo nix gnupg];
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
authenticates API reads only. The execution user's Git identity, signing and
hooks are honored when creating candidate commits.

The module creates `simit-updater-<profile>.service` and, when scheduled, its
timer. State is private and writable; workspace source and user configuration
remain read-only in the service. The selected user must already exist unless
using the module's dedicated `simit-updater` user. Services load the generated
regular Nix-store policy file directly; `/etc/simit/updates.json` remains its
operator-facing link. Provision that user's Git
identity, signing configuration, transport and credential-file access.
Each service can write only the state directory of its profile's workspace.
Workspace state directories must be disjoint, including after runtime symlink
resolution. Enrollment digests use exactly 64 lowercase hexadecimal characters.
Generator source revisions require full 40-character lowercase commit SHAs.
Inventory paths, command executables and credential files are absolute runtime
paths; credentials cannot reside in the Nix store. These constraints are checked
while rendering the workstation policy as well as at runtime.

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
`workspace simit-inventory --forge HOST=KEY` projection uses its configured tree,
sources, exclusions and canonical/mirror policy. Its lifecycle is `observed`;
select that explicitly when enrolling these observations, or supply an inventory
producer with independently declared lifecycle data.

## Commands and transaction

```sh
simit projects update --profile fleet --dry-run
simit projects update --profile fleet --push
simit projects update --profile fleet --status
```

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
   then push a maintenance ref and require exact-candidate original push qualification.
6. Reinspect source and policy, then conditionally advance the target branch to
   the same candidate using an explicit Git lease.
7. Verify exact-source default/target-branch adoption and record a new baseline.

Managed hashes identify canonical Git blobs from the staged/committed tree,
so clean/smudge filters and newline conversion do not invalidate the next rollout.
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

Existing workflow branch filters must admit `maintenanceRefPrefix`. Missing
candidate events, checks or capability support cannot authorize landing. Remote
movement, changed default branches, protection changes and managed-file edits
invalidate earlier candidate acceptance. Failed, skipped, cancelled, neutral or
rerun required cases cannot qualify. Receipts and adapter responses remain in
`stateDirectory`; `--status` reads them without discovery or forge queries.

Repository clones, receipts and baselines are isolated under a profile-specific
directory in `stateDirectory`. `--status` reports only the requested profile.
Receipt policy hashes include the active profile, its workspace, and its generator
and consumer forge records. Frozen selection keys use the profile, workspace and
generator forge policy. Changes to unrelated configuration entries therefore
preserve the frozen selection and resumable candidates; changes to a transaction's
referenced policy still invalidate its pending receipt. Frozen projects revalidate
against the currently enrolled consumer forge host before any adapter request.

Before landing, source movement archives the invalidated receipt and prepares a
fresh candidate from the newly observed source, preserving the adopted baseline.
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

Forge adapters require a store-backed executable or `adapterSha256`. File-valued
configured arguments must be absolute store paths or enrolled in `adapterInputs`
as path-to-SHA-256 entries. Digest-enrolled files are executed or read from private
verified snapshots; store inputs must resolve to files inside the store. Adapters
and generators retain their resolved store installation paths even when a digest
is supplied, preserving wrapper resources and installation-relative loaders.
Non-store snapshots remain isolated from their mutable original locations. Adapters
run from a private directory, with runtime values supplied through request JSON.
Script interpreter chains must also resolve entirely inside the store. Shebang
arguments are restricted to no arguments, Python's isolated `-I`, or
shell `-e`; arguments that can select another script or loader are rejected.
Tool execution clears runtime-injection environment variables and disables Python
user-site imports; Python scripts should use an isolated (`-I`) store interpreter.
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
authority, complete job/step success and app-bound legacy protection contexts.
Workflow source must expose its jobs and steps directly: `continue-on-error` is
accepted only when absent or the literal `false`. Expressions and reusable
workflow calls require an adapter that can establish the underlying outcomes.
This source check applies to generator and consumer qualification, and to
pre-push admission of committed candidate bytes.
Required checks from other applications, unbound contexts, active rulesets and
PR-required protections are rejected during admission. It does not migrate
protection contexts, CI engine variables, or PR-only custody contracts. Those
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

The GitHub adapter checks branch trigger filters against both qualification
branches, including ordered exclusions and tag-only triggers. Literal branch
names and `*` / `**` globs are supported; richer pattern syntax fails closed.
Path-filtered push workflows are rejected before publication; supporting them
requires an adapter that can evaluate GitHub's exact-change trigger semantics.
Admission failures leave no published candidate receipt or maintenance ref and
can be retried after correcting policy or templates.

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
