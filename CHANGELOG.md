# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- `simit init ci --prebuild-only` now accepts `--ci-provider crow` and
  `--crow-format` when `[prebuild].provider = "crow"`, and `--crow-format`
  selects that prebuild's workflow format. Both stay rejected for GitHub
  prebuilds, and `--ci-provider actions` is still accepted.
- Add `[ci].local_gate`: `simit init ci` also generates `scripts/ci-local.sh`.
  It replays the generated `ci*.yaml` run steps and `[ci].nix_builds` locally,
  without hosted-runner steps or job `env`. It supports `--list` and
  `SIMIT_LOCAL_ONLY` filtering, is drift-checked, and is retired when the
  option is turned off.
- Add `[ci.nix_build].engine_revision`, a committed exact grouped-engine
  revision used when `vars.SIMIT_CI_ENGINE_REVISION` is unavailable, such as in
  fork pull requests; the enrolled variable still takes precedence.
- Add Crow-only `[prebuild].qualify`: `simit init ci --prebuild-only` also
  renders one `.crow/prebuild-qualify-<name>` workflow each for
  `nix flake check --no-build`, every `qualify` installable and every prebuild
  `nix_builds` installable. They run on pull requests and default-branch
  pushes, without secrets, in parallel and each within the repository
  timeout. This gives projects without a GitHub mirror Nix qualification
  before the publishing prebuild.
- `[ci.nix_build].only` now coexists with `[prebuild]`: Nix-only projects keep
  their exact qualification matrix and add the Attic/archive prebuild without
  gaining language CI.
- Add `[prebuild].provider = "crow"` for projects without a GitHub mirror:
  `simit init ci --prebuild-only` renders `.crow/prebuild.yaml`, which runs
  only on default-branch pushes, builds the prebuild installables and, with
  `publish_attic`, runs `attic_app` with the `[release.attic].token_secret`
  Crow secret. Pull requests never receive the token, and CI keeps its own
  Nix build matrix.
- The GitHub prebuild emits its reusable `workflow_call`/`release: true` entry
  only with `[release.github]` or `[prebuild].release_archives`. A publish-only
  prebuild no longer requires `CHANGELOG.md` or `keys/maintainers.gpg` and reads
  the Attic token from the repository secret; release calls pass `attic_token`,
  with direct pushes falling back to the repository secret. Release workflows
  call the prebuild only when it exposes that entry.
- Add per-component `cargo_manifest` to `[[monorepo.components]]` for components
  that own a separate Cargo workspace. Their packages resolve from that
  workspace, cross-workspace path dependencies still add edges, and generated
  crate publication and the trusted publisher pass the manifest to Cargo.
- Add opt-in `[monorepo].submodules`: generated GitHub qualification and source
  publication checkouts fetch Git submodule components recursively. Output is
  unchanged when it is absent.
- With `[monorepo].submodules`, trusted crate and native publishers admit
  submodules pinned by the signed superproject tree (`--source-submodules`):
  `.gitmodules` from the signed tree, same-owner HTTPS remotes only, exact
  pinned commits, no nesting. Gitlinks remain rejected without the setting.
- Add `[release.artifacts].netrc_secret` so GitHub release and prebuild jobs
  authenticate private Nix substituters from a netrc Actions secret, written
  with mode 0600 under `$RUNNER_TEMP` before Nix is installed. Non-GitHub
  release backends reject it.
- Signed-source and trusted crate verification jobs add every required gate's
  deadline to their own 60 minutes, even when no gate retains artifacts. They
  previously stayed at 60 minutes and cancelled serial gates mid-release, or
  left the workspace lane and packaging only the slack after artifact-bearing
  gates. Generating publishers and
  hosted releases whose serial gates exceed the 360-minute job limit now fails
  with or without artifacts; independent CI gate jobs and local release
  commands are no longer subject to that limit.
- GitLab primaries: `init ci --check` now reports stale generated workflows
  (such as a former GitHub `nix-builds.yaml`) exactly as a write removes them,
  and `audit_ci` expects the GitHub prebuild generated beside `.gitlab-ci.yml`.
- Allow a `[ci].nix_builds` qualification matrix alongside `[prebuild]` when
  `[prebuild].nix_builds` selects the published subset or `[ci.nix_build]` is
  configured on GitHub primary CI; `--prebuild-only` keeps that matrix,
  `only = true` still excludes `[prebuild]`, and prebuild-only projects (and
  GitLab, Forgejo or Crow primaries) render unchanged.
- Allow `publish_crates` with Nix-only CI. Nix-only Cargo workspaces receive the
  per-member signed-tag publish workflows, paired trusted publishers and release
  trust root without language CI workflows, and their tag workflows omit
  `nix flake check` when the primary CI does. Dependent members require the
  dependency-ordered coordinated strategy.
- Retain optional required-gate diagnostic artifacts in GitHub CI, signed-tag
  source builds and release verification, including failed gates but not skipped
  ones. Preserve runner compatibility, reject action-trimmed input whitespace,
  resolve effective backends after overrides, reserve CI and shared publishing
  upload time, retain selected hidden and absolute-path evidence, reject artifact
  authentication and step-local environment contexts, and keep diagnostics
  separate from immutable archives and reusable release-prebuild artifacts.
  Reject ambiguously rooted leading expressions in isolated verification instead
  of silently collecting from the wrong checkout.

### Changed

- Deduplicate Simit's own generator qualification and native/Nix validation;
  retain Nix-only checks with the production build and lighten the Rustdoc shell.
- Raise Simit's minimum Rust version to 1.88 to use the security-patched Hickory
  DNS resolver; generated projects keep their independently declared minimums.
- Reuse the pinned grouped-build engine's closure across runs through the
  GitHub Actions cache instead of compiling Simit in every group job;
  provisioning and its revision/digest evidence are unchanged.

### Added

- Add `[prebuild].push_branches` to limit the native prebuild's branch pushes;
  pull requests and release `workflow_call` are unchanged, and Simit's own
  macOS prebuild now runs on pull requests and `trunk` pushes only.

- Reuse declared heavy Nix inputs of grouped GitHub matrices across runs through
  `[ci.nix_build].cache_installables`, keyed by exact derivation and refusing any
  cached closure that would satisfy a qualification output.
- Explain complete operator enrollment with `simit projects update --plan`,
  including missing, disabled and filtered canaries, without forge requests,
  credential reads or candidate generation. Bind the report to policy and
  inventory digests while retaining execution-time canary admission.
- Workstation-owned updater profiles through `simit projects update`,
  `lib.mkUpdateConfig` and `nixosModules.updater`, with isolated primary-CI
  candidates, exact-source qualification, conditional landing and resumable
  adoption receipts. Runtime tools are configurable and default to Git.
- A lightweight `devShells.<system>.test` for native tests without the default
  documentation, website and pre-commit provisioning.
- Native `simit ci observe-original` and `verify-original-retention` capture
  enrolled original GitHub producers with bounded transient GET retries, one
  aggregate deadline, continuous identity binding, explicit incomplete receipts,
  complete required job/step admission, and independent uploaded ZIP/retention
  revalidation. No producer execution or rerun API is exposed.

### Fixed

- Bind receipt-producing ungrouped Nix builds to the exact pull-request head and
  source repository, matching grouped builds instead of recording a merge commit.
- Preserve build-only checks, workflow lint and release/flake regeneration on
  every primary CI push branch when consolidating validation ownership.
- Keep push and pull-request generator qualification separate so one event
  cannot cancel the other's required check.
- Qualify generated native prebuild push runs against exact source job/step
  identities: require release-only skips, complete native matrix execution and
  successful default-branch publication while rejecting unknown predicates.
- Keep managed README badges outside fenced code examples, including shell
  comments that resemble headings beneath an HTML banner.
- Admit the path/repository/branch-isolated concurrency emitted by aggregate
  primary CI and the full-ref form emitted by separate-event Rust CI. Require
  aggregate workflows to exclude tags and retain rejection of unknown groups,
  dynamic cancellation and per-job concurrency.
- Keep local monorepo gate process creation interruptible by normal termination
  signals until native child cleanup can take ownership.
- Use job Bash for generated monorepo gate supervision on hosted and custom runners,
  retain diagnostics on gate spawn failure or abrupt native termination and
  propagate gate exit codes. Keep the tool PATH while
  locating Bash in Nix sandboxes before restoring the gate's configured PATH.
- Preserve configured monorepo gate environment data across Bash and Nix startup,
  remove consumed provider aliases and reject read-only and dynamically managed
  Bash names and startup-sensitive `BASH_ENV` before execution. Require private
  supervisor directories and bound provisioning (including executable startup),
  native initialization and gate process creation so failed or
  timed-out checks retain their diagnostic reserve.
- Limit COPR staging cleanup to generated source RPMs, preserving project files
  when the staging directory also contains specs or sources.
- Clarify NuGet v2/v3 Chocolatey source configuration and reject authenticated
  registry reads explicitly without exposing publication credentials.
- Discover NuGet v3 package-content resources for Chocolatey byte verification
  and omit SemVer build metadata from normalized package filenames and lookups.
- Reject traversal in COPR spec output paths and enter AUR clones before dropping
  root privileges, preserving private workspace permissions and clone ownership.
- Persist and verify source-bound Chocolatey candidates before Actions
  publication, retain normalized package filenames, isolate prebuild concurrency
  by fork repository, and reject colliding COPR spec/Makefile outputs.
- Honor configured COPR Makefile and SRPM staging paths, match the RPM spec's
  source archive basename, and reject missing, empty, or duplicate SRPMs.
- Give disposable COPR release fixtures a repository-local Git identity so
  production-package tests also run in identity-free Nix sandboxes.
- Link managed APT badges to the configured public repository URL.
- Generate AUR metadata as an unprivileged user on root release runners;
  restore clone ownership on success or failure and preserve prior metadata
  when generation fails.
- Run Chocolatey release publication in the project's declared environment;
  retain source-bound packed candidates for byte-verified retries, reject
  conflicting registry content, and keep publication credentials out of
  packing, lookups, and command output.
- Honor monorepo push-branch filters and select binary unit tests when a Rust
  CI selection has no library, including aggregate and Nextest workflows.
- Preserve per-member Crow MSRV checks under Omnix replacement and run split
  audit/deny setup and checks on their selected quality runner.
- Carry CLI cancellation through every source Git probe and launch probe children
  with a clean environment; document the required isolated observer-job boundary.
- Revalidate captured attempt-one jobs and log contents against GitHub during
  retention; an artifact digest cannot authenticate a recomputed forged receipt.
- Bind original custody to complete logs from validated attempt-one job IDs,
  tolerating only transient redundant step-file ZIP membership while preserving
  exact shared contents, complete aggregate coverage and all retention bounds.
- Reserve full partial-download retry bodies independently of fresh metadata and
  log revalidation, without resetting the retention deadline.
- Preserve `@` in workflow filenames and refs, rejecting ambiguous producer paths.
- Keep DNS resolution asynchronous and owned by each transfer runtime, including
  cancellation, deadline expiry and subsequent calls through a reused client.
- Charge local retention inventory, archive validation and provider reads to one
  five-minute deadline starting at entry, preserving the hosted step allocation.
- Reject blocking or substituted event/spec input files during bounded preflight;
  install CLI cancellation early and retain incomplete preflight diagnostics.
- Verify source commit, tree and blob object hashes before admitting observer policy.
- Admit Git-valid Unicode workflow refs and retain an incomplete receipt when
  cancellation arrives during final receipt publication.
- Keep protected source probes usable inside Nix sandboxes, bind PR heads to the
  protected merge revision, isolate observer TLS/proxies from job environment,
  and retry bounded transient request/protocol failures.
- Honor reset-only GitHub rate limits, bind observer event targets to the native
  runner ref, and resolve source probes through protected platform Git paths.
- Keep observer source probes free of external clean filters and observer
  credentials, and support synchronous transport calls from Tokio tasks.
- Repeat original-producer selection during retention, preserve the event snapshot
  that authorized source enrollment, fall back from empty primary tokens, and
  register native Windows console cancellation for the observer CLI.
- Original-producer custody can interrupt stalled header/body transfers while
  preserving partial bytes, admit repository-name period pairs and optional
  attempt-scoped job fields, and bind clean-filtered checkouts to the authoritative
  committed enrollment policy, including CRLF and maximal CLI spec inventories.
- Bind original-observer enrollment bytes to the source commit, reject enrollment
  of the active observer, and use portable artifact member paths on Windows.
- Filter same-SHA producer discovery by branch/PR target, validate provider
  workflow refs independently, and allow large downloads the remaining deadline.
- Reserve generated custody evidence, retry capped transient HTTP error bodies,
  and preserve packet directory extensions in independent retention diagnostics.

## [0.21.0] - 2026-10-15

### Added

- Bounded runtime admission through a reusable library and `simit runtime admit`:
  explicit retryable statuses, cancellable backoff, per-probe/overall deadlines,
  aggregate byte budgets and fresh observation receipts. CI and admission reuse
  the same process/cancellation implementation; existing CI interfaces remain
  compatible. Systemd adapters and escaped-descendant cgroup containment remain
  consumer-owned qualification requirements.
- Explicit `sealed-v2` completion custody: immediate per-original capture,
  native step-log completeness admission, immutable artifact ID/digest verification
  and independently uploaded aggregation with fresh native revocation checks.
  Existing policies retain their original protocol.
- A mixed-language monorepo component graph for changed-path qualification,
  member discovery, shared formatter composition, and native GitHub CI.
- Independent Cargo release planning, package-scoped version bumps, verification,
  tag sync, and publication through signed `<package>/v<version>` tags.
- Separate credential-free component builders and protected default-branch
  Cargo publishers, binding the complete signed native tag, an independently
  enrolled package namespace, original provider run, and archive checksum.
- Component-scoped Git release notes and automatic changelog drafting, with
  package-qualified compare links and history bounded by owned paths.
- Independent static Python/npm version owners, package-qualified tags, scoped
  lock updates, verification and prerequisite-gated tag synchronization.
- Independent signed-tag Python/npm publication with complete native component
  qualification, source-bound artifact receipts, exact registry checksum conflict
  detection, private-package exclusion and bounded propagation verification.
- Opt-in `[ci].separate_events` for GitHub Rust CI, preserving independent push
  and PR originals in aggregate workspaces while retaining default coalescing.
- Native dependency export-stream admission with importer-compatible NAR bounds,
  exact closure/reference binding, and one-use private descriptor restoration.
- Native `simit ci nix-group` and library execution contracts replace embedded
  grouped-build Python and its Python test harness. Enroll an immutable
  `SIMIT_CI_ENGINE_REVISION` repository variable before adopting regenerated
  grouped workflows. Retain v1 receipts, shell-only failure diagnostics and
  materialize-plus-rebuild execution for fresh members.
- Opt-in `[ci.nix_build.system_runners]` for native exact-installable matrices.
  Route each system-qualified gate to its declared runner, reject missing or
  ambiguous mappings before writes, verify the native Nix architecture, and
  restrict hosted KVM setup to x86 Linux rows.
- Opt-in completion-driven custody for enrolled original branch/PR producers,
  with bounded log/artifact capture and idempotent producer-source status reports.
- CI-scoped public Nix cache configuration and bounded same-run build-input
  closure transport with exact source/run/attempt/derivation provenance.
- Explicit fresh qualification installables, retaining runtime execution while
  restoring only build dependencies.
- Opt-in shared-store Nix build groups with complete installable assignments,
  discovered-check coverage admission and per-output failure evidence.
- Independent Rust, full-flake evaluation and optional realization lanes for
  GitHub-hosted Rust/Nix projects, retaining all existing quality steps.
- Opt-in `[ci.nix_build].push_tags` patterns to retain tag-push qualification
  alongside branch and pull-request gates, with ordered exclusions and stable
  configuration regeneration. Reject exclusion-only filters and keep tag refs
  separate from same-named branch/PR cancellation groups.

### Fixed

- Omit the default empty tag-prefix argument from trusted Cargo publisher workflows,
  preserving compatibility with enrolled engines that predate prefixed releases.
- Publish sealed capsule names only after admission succeeds, keeping failed-capture
  diagnostics separate from reusable capsules.
- Preserve explicitly selected custody protocols when persisting CI option changes.
- Require static native release policy for historical verification, reject Cargo
  changelog symlinks, locate historical Cargo manifests after directory moves, and
  bound Git notes by arbitrary admitted SemVer prerelease tags.
- Recover historical native publication policy from tagged configuration, include
  Cargo owners in changelog collision checks, resolve implicit Cargo paths before
  native ownership validation, and recheck release tags after qualification.
- Authenticate fixed-host, nonredirecting npm metadata reads and retry transient
  propagation failures while preserving immediate checksum-conflict rejection.
- Preserve preceding changelog tag spelling on prefix adoption, respect disabled
  component Cargo publication, and reject ambiguous selectors/shared native changelogs.
- Keep npm wildcard ranges classified when later numeric fields exceed safe integers.
- Reject npm wildcard ranges followed by numeric components as distribution tags
  and enforce npm's safe-integer core version limits at enrollment and admission.
- Respect tagged native publication eligibility during historical verification and
  conservatively qualify the full graph for Git paths outside the component grammar.
- Verify historical native package names from their namespace-owned tag and reject
  unsupported private Python version labels before lock handling or mutation.
- Enforce configured JetBrains release tag prefixes and validate manual versions
  through environment values rather than interpolating them into shell source.
- Verify historical native releases using their tagged registry channel and apply
  explicit component gate variables after entering the Nix development shell.
- Share exact signed-tag-name admission across Actions release validators,
  including comprehensive releases; reject unsupported Crow note/prefix modes.
- Bind direct VS Code publication to the signed tag object's internal name before
  marketplace credentials are loaded, rejecting cross-component signature aliases.
- Reject Crow release workflows with required gates before generation rather than
  silently omitting prerequisite checks during publication.
- Require the admitted npm distribution tag to resolve to the selected version
  during registry verification, including the default latest channel.
- Verify selected Cargo name/version from the component tag's manifest, including
  workspace-inherited versions, rather than only checking the tag ref exists.
- Reject component version bumps excluded by local dependency requirements before
  mutation, including renamed workspace-inherited dependencies.
- Preserve primary-only scope when inferring monorepo drift with independently
  managed VS Code or JetBrains publishers.
- Require independent crates.io publishers' complete local dependency closure to
  allow crates.io, rejecting alternate-only dependencies before workflow writes.
- Namespace monorepo qualification concurrency by source repository so identically
  named fork branches do not cancel one another's runs.
- Reject non-object npm publishConfig at enrollment and protected admission.
- Require custom treefmt imports to participate in the returned evaluated module,
  rejecting unused nested expressions while accepting formatted direct wiring.
- Reject unsupported independent Cargo publication prereleases before any gate,
  version edit, commit or tag, including bumps from existing prereleases.
- Propagate configured tag prefixes into VS Code publication triggers and exact
  version validation on GitHub and Forgejo.
- Preserve older npm-supported leading-hyphen package names during enrollment,
  packing and protected upload while retaining separate Git namespace validation.
- Reject leading-hyphen release namespaces before generating or mutating a
  release, preserving Git-valid underscore and alphanumeric namespaces.
- Bind protected publication to the signed tag object's internal name, rejecting
  aliases that reuse another component's valid signature at the same version.
- Isolate hosted release gates from artifact workspaces; release jobs re-checkout
  and revalidate the immutable signed source after prerequisite jobs succeed.
- Report protected Python/npm publisher credentials as locally unverifiable even
  when the selected release is already present in its registry.
- Preserve unset npm publication access so releasing an existing restricted
  package does not change its visibility; provenance checks existing visibility.
- Reject workflow expressions in component gate environments before planning or
  generation, preserving literal local/hosted gate parity.
- Preserve primary-only scope in component CI self-checks so independently
  managed extension publishers do not block qualification.
- Skip crates.io registry and token verification for Cargo packages whose publish
  policy excludes crates.io, using the same eligibility as publisher generation.
- Build native publication archives on a fixed hosted runner and independently
  require that builder during protected publisher admission.
- Reuse native retry archives only from successful original hosted-builder
  attempts, rejecting failed-attempt forgeries and ambiguous artifact names.
- Run component prerequisites before Cargo release mutation and required gates
  before credentialed prebuild publication; share the effective notes policy.
- Update nested npm workspace lock records, support explicitly owned root Cargo
  sources, and resolve native-only repository trust without Cargo metadata.
- Preserve prefixed Homebrew release URLs and require the treefmt wrapper in
  dev-shell package bindings rather than unrelated package declarations.
- Honor explicit native verification backends and supported prerelease Git-note
  ranges; skip unrelated ancestor npm locks and reject invalid dist tags during
  publication enrollment.
- Revalidate Cargo component source identity after qualification, skip unrelated
  ancestor uv locks, validate namespace length during admission, and preserve
  consumer npm lifecycle metadata without executing it in the publisher.
- Carry tag prefixes through reusable prebuild and PyPI workflows, reject
  SemVer-range npm distribution tags, update every owning uv workspace lock,
  and verify each component matrix runner's native Nix system before its gates.
- Use the effective release-notes policy in reusable prebuild validation, so
  Git or disabled notes do not require a changelog in the called workflow.
- Require complete unyanked Python registry inventories during verification,
  validate npm provenance types before enrollment, and reject unsupported
  monorepo extension publication before workflow reconciliation.
- Resolve formatter toolchains from active lexical Nix bindings instead of
  comments or strings, preserving existing crane/overlay toolchains.
- Admit npm registry names before enrolling publication, reconcile signed npm
  distribution tags on archive recovery, qualify every component for undecodable
  Git paths, and normalize Cargo workspace member paths on Windows.
- Omit independent crates.io publishers for alternate-only or unpublished Cargo
  members while retaining their component qualification.
- Bind native retry archives to the original hosted upload step's provider ID and
  digest, rejecting same-name replacements made by later qualification attempts.
- Preserve already-normalized Python prerelease identities at protected upload,
  validate public distribution names and npm private flag types before enrollment,
  and qualify byte-path planning without materializing invalid APFS filenames.
- Honor the documented absolute `XDG_DATA_HOME` project-registry override on
  macOS and Windows as well as Linux, retaining native defaults when unset.
- Preserve `SIMIT_NIX_FRESH` in environment-mode grouped execution, retain
  `SIMIT_NIX_FRESH_INSTALLABLES` as an alias, and reject conflicting values before
  execution so freshness requirements cannot silently disappear.
- Reject all pre-existing numbered member evidence before grouped Git/Nix
  observations or evidence writes, including interrupted/fallback-only attempts,
  out-of-range indices, files and dangling symlinks.
- Bound complete grouped workflow files to GitHub's 500 KB limit before writes,
  require an explicit nonempty runtime evidence directory, reserve transport
  environment keys, and reject Actions expressions in retained grouped hooks.
  Bind secret expressions through `extra_env` and use shell variables in hooks.
- Reject grouped execution specs above the shared 16 MiB serialized JSON budget
  before generation writes or runtime effects, counting JSON escapes while
  preserving the individual 1 MiB hook limit.
- Enforce grouped stdout/stderr limits before evidence writes, execute accepted
  large hooks from retained scripts, and make output-reader startup failure-safe.
  Deny NAR case-hack aliases before import and budget restore argv/environment
  bytes before any store mutation for portable closure verification.
- Keep generated group specs and diagnostic fallback hooks out of argv/environment
  strings, preserving Bash command semantics for hooks through the 1 MiB limit.
  Retain inherited-pipe/cancellation diagnostics, platform exec headroom and the
  transport producer's checked zlib compressed envelope.
- Preserve Unix cancellation and signal exit statuses across grouped execution,
  retain admission diagnostics, and force fresh rebuilds onto the current worker.
  Bootstrap uses public credential-free Nix fetching and portable hashing, with
  unsupported Intel macOS runner admission rejected before generation writes.
- Admit dependency attributes outside the actual checks namespace, apply CI cache
  policy on preinstalled Forgejo Nix, and explain full reruns for attempt-bound
  transport. Reject Darwin case-hack aliases before import and bind checksum and
  decompression to one private compressed snapshot.
- Verify native exact-build runners against OS/CPU identity independently of
  Nix's configurable `eval-system` and `system` settings, preventing a configured
  system override from admitting the wrong hardware architecture.
- Preserve native runner mappings alongside shared-store groups: route each
  homogeneous group to its declared system, retain the hardware guard and scoped
  KVM setup, and reject mixed-system groups or unsupported native dependency
  transport before writing configuration or workflows.
- Select exact-build action versions from mapped workers, support standard
  two-segment system derivation outputs, and recognize unambiguous PowerPC uname
  aliases while preserving independent hardware verification.
- Preserve explicit Nix output selectors when routing standard system outputs
  to native workers, extracting only their attribute path for system validation.
- Grant check-suite read access only to completion-custody collection, bound the
  fully rendered observer to 500,000 bytes before writes, and ignore unenrolled
  workflow paths that reuse producer names without changing custody statuses.
- Bind completion-custody workflow hashes to GitHub's native executed-file
  revision, independently of raw PR heads or producer checkout revisions, with
  fresh proof revalidation before capture completion and after upload.
- Bind completion custody to one validated PR/base target and observed snapshots,
  derive case-safe collection queues from that identity, validate composed status
  contexts, and reserve bounded upload/report time before publication.
- Normalize completion-custody repository casing, bind status contexts to the
  exact branch, and bound aggregate downloads, retained bytes, archive expansion,
  inventories and capture time before source-head success can be published.
- Preserve completion-custody source/attempt admission, capture budgets and
  failure revocation when Python optimization is enabled.
- Bind the full dependency export stream's actual path/reference inventory to the
  admitted closure before canonical-store import, using a bounded non-extracting
  NAR parser and one private spool descriptor through validation and import.
  Align NAR depth, entry-name and symlink-target bounds with the Nix importer
  so invalid later records reject before any preceding record can be imported.
- Keep dependency transport's source, provenance, size and qualification-closure
  fences active under Python optimization; provision the dependency producer's
  interpreter on custom runners as well as hosted workers.
- Migrate recognized inline MSRV bindings in existing canonical flakes before
  hook-wiring shortcuts, and detect that required source migration in full checks.
- Generate MSRV toolchain manifests as source assets so no-build, read-only Nix
  evaluation succeeds on cold stores without changing the declared compiler.
- Preserve each grouped installable's hook context, including shell-only
  diagnostics after setup or preflight failures, and retain unset timeouts.
- Qualify one immutable PR head across independent lanes and required gates,
  evaluate all flake systems, preserve Cargo tool paths in every lane and reject
  unsupported step-runner overrides before normalization.
- Check out signed Git symlinks as inert target-text files during independent
  archive reconstruction, admitting unrelated tooling links without following
  them. Continue rejecting submodules and any Cargo manifest, declared input,
  lockfile or archive member that depends on a Git symlink.
- Keep the independently checked-out publisher engine in its own Cargo
  workspace, so it can build beneath a candidate workspace without joining it.
  Retain single-crate resolution and independent regression fixtures.
- Render long trusted-publisher dependency lists as formatter-stable block
  sequences, preserving dependency order and short inline arrays.
- Use immutable Node 24 action revisions on recognized GitHub-hosted runners
  and credential-free release workers. Preserve the Node 20 action baseline
  for private runner labels and Forgejo, together with signed-tag admission,
  independent trust roots, and archive custody verification.
- Select the compatible action runtime for every monorepo component's native
  runner matrix; require recognized hosted workers and reject candidate
  authentication contexts before generating independent Cargo builders.

## [0.20.0] - 2026-10-11

### Security

- Separate signed-tag archive builders from default-branch, environment-only
  crates.io upload workers. Bind admission to the independently qualified
  publisher revision, enrollment, signer, tag, source, run and attempt; rebuild
  and verify archive custody on fresh hosted workers before credentials enter.
- Reconstruct archives from absolute paths and private Cargo state, reconcile
  uncertain uploads by exact registry checksum, and require frozen dependency
  resolution before dependent publication. Reject unsafe environment names in
  configuration and direct renderer calls.
- Require default-branch-restricted environment-only policy App credentials,
  reject unprovisionable GitHub secret names, and allow explicitly credentialed
  external reusable-controller runs to publish their exact-head report.

### Added

- Opt-in `[ci.nix_build].separate_events` for independent push and pull-request
  qualification without cross-event cancellation; ordinary coalescing remains
  the default.
- Project-owned Actions workflow templates and literal variables, sharing CI
  generation, configuration persistence, drift checks, and registry ownership.
- Typed explicit Nixpkgs package/test additions unioned with upstream discovery,
  including empty system mappings, with request-bound package-scoped broken
  warnings and frozen selection/test identity validation.

- Project-owned Rust CI commands through `[ci].check_command`, optional
  `nix_flake_check`, and opt-in Nix-runtime Cargo caching for Actions workflows.
- Scoped GitHub Nix build-matrix options under `[ci.nix_build]`: exact-only
  flake qualification, private-input environment/preflight and setup, KVM,
  bounded realization, and retained source/output metadata, build diagnostics
  and project-exported receipts.

### Fixed

- Isolate multi-system Nix flake-check concurrency by source repository so forks
  with matching branch names cannot cancel each other's qualification, while
  retaining same-repository push/PR coalescing.
- Exclude tag pushes from installable qualification matrices, retain release
  workflow triggers, and use current pinned GitHub checkout and evidence-upload
  actions in focused qualification.
- Use the branch filter alone for installable qualification so consumers enforcing
  branch-only CI can validate it without accepting redundant tag filters.
- Reserve release-owned paths from project templates and keep template contents
  out of built-in CI inference; qualify durable-branch updates and reject
  committed generation drift after retaining its evidence.
- Reserve every release-registry filename, reject generated template sources,
  include untracked outputs in production parity, and isolate fork qualification
  concurrency by source repository as well as branch.
- Reject opposite-platform generated template sources, enforce effective CLI
  backend constraints in check and write modes, and report missing built-ins as
  ordinary drift even when only project template outputs remain.
- Identify template outputs by configured path even after ownership-header edits,
  and reject case-insensitive collisions with built-ins, releases, or other templates.
- Require standalone ownership headers in the workflow preamble when retiring
  generated files; preserve hand-written workflows quoting marker strings in YAML
  bodies while recognizing moved headers, document separators, and Jsonnet comments.
- Preserve template retirement through commented YAML document starts and YAML/TAG
  directives; retain an executable project-audit repair command when only templates
  remain, without inferring builtin policy from their bodies.
- Reject control characters, YAML-forbidden BMP noncharacters and Unicode line
  separators in template source/output paths before writes so ownership comments
  cannot introduce malformed generated workflows.
- Keep the default development environment usable when a worktree lacks the
  optional sibling language-server checkout.
- Wait for exact crates.io dependency resolution through Cargo after each
  coordinated workspace publication, including checksum-based resumes, before
  starting dependent jobs. Fetch and verify lockfile evidence for library and
  binary-only crates, with bounded retries and individual calls; crates.io web
  API readiness alone no longer advances the publication plan.
- Generate native Rust, cross Rust and Python flakes against one qualified Harbor
  monorepo revision and its namespaced APIs. Keep MSRV toolchain manifests in
  tracked source, preserve existing Rust library namespaces when patching hooks,
  and recognize treefmt-normalized inherited hook bindings.
- Normalize Python registry names and prerelease versions, keep local uv lock
  records aligned, and reject unsupported public version/publication settings
  before version mutation. Omit empty component-step environment mappings so
  generated qualification and native-publication workflows pass Actions linting.
- Honor an absolute `XDG_DATA_HOME` registry override on macOS and other native
  platforms, keeping project fixtures isolated from the platform default home.
- Bound native Nix prebuilds to one build job, two cores, and three hours while
  streaming builder logs for package-test diagnostics.
- Stream Nix builder logs while bootstrapping monorepo qualification and
  publication shells so native package-test failures retain their diagnostics.
- Resolve public GitHub and Codeberg SSH inputs over HTTPS before monorepo
  qualification, including the full native graph used for package publication.

- Include Nix builder-phase output in captured build artifacts so hosted
  cancellations retain build diagnostics alongside client messages and results.
- Give native GitHub Nix flake matrices read-only permissions, coalesced push/PR
  concurrency, bounded parallelism and timeouts, and explicit build-resource limits.
- Resolve and check out fork PR heads from their source repository while keeping
  review plans and PR reporting anchored to the base repository.
- Keep GitHub review workflows out of ordinary CI provider inference so Forgejo
  and Crow projects remain auditable, including review-workflow drift checks.
- Reject unsupported language checks and release requests in Nix-only CI instead
  of silently omitting them; honor and persist explicit `false` overrides.
- Allow Python flake projects to select exact-only Nix qualification without
  generating language CI or requiring language-specific version metadata; keep
  generated-workflow checks and registry audits aligned.
- Coalesce configured hosted Nix qualification runs for branch pushes and pull
  requests so the same source does not consume two hosted build matrices.
- Isolate scoped Nix qualification concurrency by source repository, preserving
  same-repository push/PR branch coalescing across unrelated fork branch names.
- Reject a coordinated publication when the refetched signed tag resolves to a
  different commit than the immutable workflow event SHA, keeping validation
  and all later release jobs bound to the same source tree.
- Order versioned workspace dev-dependencies before their dependents during
  coordinated publication, because Cargo resolves them while packaging the
  lockfile. Path-only unpublished test helpers remain excluded from the plan.
- Read explicitly dynamic Python versions, scripts, and optional extras from Poetry metadata when a project also declares PEP 621 metadata. Preserve static PEP 621 fields and uv dependency groups, use the same version provider for PyPI release-tag validation, and report unsupported dynamic version providers clearly.
- Bind signed-tag publication to independently fetched default-branch keys and
  the exact checkout before evaluating package metadata, including per-member
  publishers; explicitly pin every coordinated release job to the immutable
  event SHA and regenerate this repository's publisher from that renderer.
- Give per-member GitHub/Forgejo jobs distinct package-qualified check names,
  including split checks and integration gates; restrict GitHub crates.io
  publishers to signed semver tag pushes, with retries on the original run.
- Validate signed event-bound tags before VS Code credentials, reusable release
  prebuilds and Windows builds/publication; keep all dependent jobs on the same
  immutable event checkout and document retrying the original tag-push run.
- Execute a declared flake formatter's exported executable, using `--ci` for
  treefmt and no arguments for custom formatters; reject repository mutations
  and fail on broken formatter exports.
- Preserve formatter shell variables and Nix interpolation through Crow's
  literal-dollar preprocessing in YAML and Jsonnet workflows.
- Authenticate release verification fetches explicitly without persisting checkout
  credentials; cover private fetches and shallow reusable prebuild tag retrieval.
- Dispatch reviews through an explicit branch or tag resolving to the reviewed
  controller SHA, reject ref movement before checkout, and declare the optional
  reusable-workflow report credential.
- Honor cancellation when collecting Nix diagnostics while retaining collectors
  after build or setup failures.

## [0.19.0] - 2026-09-30

### Added

- `simit test --git-fixtures -- <command>` provides disposable, directory-scoped Git fixture isolation while preserving checkout hook/signing policy and child exit status.
- Project-owned Crow check commands, with an optional push-only secret that is excluded from pull-request steps and preserved during CI configuration updates.

### Fixed

- Select the flake's configured treefmt wrapper before bare shell tools in Nix formatting gates, preserving project policy and formatter failure status.
- Recognize the current `harbor-py` input name alongside `py-harbor` when validating custom Python flakes.
- Keep generated plugin publisher scripts lint-clean by emitting only the configured credential variables and identifying jq's literal filter variables.
- Resolve GitHub SSH flake inputs over HTTPS in hosted Nix jobs so public pinned dependencies remain usable without runner SSH keys.
- Apply the same GitHub input transport setup to comprehensive artifact release workflows.
- Keep activated multi-channel release scripts lint-clean, report concrete missing archive versions, propagate Chocolatey filter failures, and exclude the Windows ZIP output from its own inputs.
- Keep Nix CI formatting usable for legacy shells without treefmt while preserving full uncached treefmt checks and propagating formatter failures whenever it is available.
- Reconcile obsolete generated Rust workflows across providers during CI writes, matching check-mode ownership while preserving handwritten CI and release workflows.
- Match coordinated publication in registry audits instead of reporting a missing per-crate publisher after successful generation.
- Quote coordinated-publication archive paths without shell glob expansion so generated workflows pass ShellCheck.
- Ignore untracked build outputs during Git-backed language detection so documentation builds cannot change formatter policy or README badge checks between local and hosted CI.
- Honor test and Clippy feature policy in Nix and split-runner Actions workflows, including library-only passes and crates.io publishers.
- Run MSRV checks with the declared compiler in both CI and publishing: install it for Cargo jobs, verify the dedicated Nix shell's compiler, and generate matching MSRV shells for new Rust flakes.
- Scope GitHub Actions concurrency by workflow path so workspace members with the same display name cannot cancel one another.
- Keep the Pages build output link for artifact upload and domain validation, while preserving custom installables inferred from older workflows.
- Validate the treefmt hook's enabled state, uncached wrapper command, and full-file scope structurally; reject disabled, redirected, filtered, or shadowed hooks instead of accepting textual decoys.
- Use treefmt as the sole generated formatter when selected, and require recognized policy coverage before hooks-only updates remove standalone formatters.
- Recognize generated treefmt policies structurally in checks and registry status, and use the actual Cargo workspace edition instead of assuming 2024.
- Generate uncached treefmt checks for local hooks and Nix-backed GitHub Actions, Forgejo Actions, and Crow jobs.
- Validate treefmt import arguments against required module parameters; reject missing arguments, unsupported dynamic calls, and misleading unrelated bindings.
- Declare GitHub Actions as this repository's CI provider and retire the generated Crow workflow.
- Keep coordinated workspace publication executable and resumable with isolated publishers, Nix job environments, and version-specific checksum verification.
- Build documentation and the project site without requiring fleet-managed compiler-cache transport on hosted runners.
- Keep Crow and treefmt argument validation compatible with the declared Rust 1.85 compiler.
- Omit Intel-macOS flake outputs unsupported by the pinned nixpkgs, and keep formatter tests independent of the operator's runner configuration.

## [0.18.1] - 2026-09-26

### Fixed

- Pages inference is provider-aware: a GitHub-hosted `pages` workflow is now
  identified positively from its own markers and workflow directory, and
  resolves as applicable configuration instead of failing with a Codeberg
  inference error that described healthy GitHub setup as malformed. Mixed
  provider claims (both marker sets, or markers contradicting the workflow
  directory) stay errors, and a Codeberg workflow with a bad repository still
  reports its original inference error.
- An `infer_expected_ci_files` configuration that will not load is now
  reported, including the offending keys, instead of silently falling back to
  defaults and surfacing only the resulting file drift.

## [0.18.0] - 2026-09-25

### Added

- Support custom non-Rust and non-Python flakes with a generic backend, including JavaScript and TeX formatter detection.
- Declarative `[[ci.required_gates]]` for project-owned custom flakes (stable id, single-line run, bounded timeout, scoped env) rendered as dedicated required jobs in CI and as prerequisite jobs in coordinated publishing.
- Opt-in coordinated workspace publication (`publish_strategy = "coordinated"` / `--coordinated-publish`, GitHub only) generating one `publish-workspace.yaml` in `simit release plan` order with signed-tag, lockstep, gate, checksum-conflict, and bounded-propagation gating.
- `docs/integrations/chaosbox-v1.md` pinning handoff with exact config, commands, gate/packaging semantics, migration, and tested fixtures.
- `simit commit`/`release --workspace` now bump `[workspace.package].version`
  (and matching `[workspace.dependencies]` requirements) for workspaces whose
  crates inherit their version via `version.workspace = true`, instead of
  failing on the non-literal per-crate `version`.

### Fixed

- Validate formatted pre-commit hook files against the project-selected component set.
- Release-plan ordering now ignores dev-dependencies (Cargo parity): publishable crates may dev-depend on `publish = false` helpers without failing the plan; normal/build/optional/target-specific path deps still order and still fail on non-publishable members.
- `--dry-run-package` output is labeled archive-construction-only (`--no-verify`), never presented as verification, dry-run publication, or publish.
- Required-gate ids that collide after job-name sanitization (`a_b` vs `a-b`) are rejected instead of emitting duplicate `gate-*` jobs.

## [0.17.15] - 2026-08-13

### Fixed

- Isolated GitHub Actions concurrency between push and pull request workflows.

## [0.17.14] - 2026-08-13

- Delegate GitHub flake-input publication to rs-harbor and allow the generated
  prebuild workflow alongside Forgejo or Crow CI.

### Added

- GitHub and Crow CI workflow generation.
- Nix installable build matrices and prebuilds for hosted GitHub runners.
- Prebuilt release archives and actionlint validation.
- PyPI trusted publishing support.
- Publishing of flake inputs through rs-harbor.

### Changed

- CI generation now validates provider capabilities and target configuration across render paths.
- Legacy generated-workflow markers are migrated automatically.

### Fixed

- PyPI release tag validation and version lookup isolation.
- Generated workflow concurrency groups now refresh and remain isolated.

## [0.17.13] - 2026-07-28

### Fixed

- Keep generated multiline release commands free of trailing whitespace.

## [0.17.12] - 2026-07-28

### Added

- Generate one aggregate GitHub Actions workflow for multi-package Cargo workspaces.
- Preserve supplementary GitHub workflow fixtures in the published flake source.

### Fixed

- Keep public Simit flake checks independent of Canix-managed cache transport.

## [0.17.11] - 2026-07-28

### Added

- Draft validated Keep a Changelog entries from Git history with `simit changelog draft` and optional project guidance.
- Publish the harbor-rs static binary bundle from tagged GitHub releases with runner capacity gates and signed provenance.
- GitHub prebuild releases.
- Documentation for automatic changelog drafting.

### Changed

- CI workflows migrated to Crow on CodeFloe.
- Crow Nix workflows now default to the devenv image.

### Fixed

- Keep harbor-rs's bundled sccache library portable when consumed through a locked GitHub flake input.
- Validate tagged versions against the release artifact package exposed by the flake.
- Allow public release runners to build cache misses without Canix's managed sccache transport.

## [0.17.10] - 2026-07-27

### Fixed

- Keep empty APT repository directories tracked so a freshly bootstrapped
  Codeberg Pages workflow can deploy before the first release.

## [0.17.9] - 2026-07-27

### Fixed

- Bootstrap Scoop buckets from multi-package workspaces by using the same
  representative-package selection as other workspace-level channels.
- Keep empty APT repository directories tracked so a freshly bootstrapped
  Codeberg Pages workflow can deploy before the first release.

## [0.17.8] - 2026-07-27

### Added

- Add provider-aware GitHub Actions and Crow CI templates, including Crow YAML
  and Jsonnet workflows under `.crow`.
- Add `[release.artifacts].prebuild_binaries` as shorthand for the
  conventional harbor-rs `release-bundle` output, while preserving explicit
  `nix_bundle_attrs` overrides.
- Add repository-backed APT Pages bootstrapping and `dist apt build`,
  `dist apt publish`, and `dist apt verify` commands.
- Add explicit per-channel release publisher policies for staged, required,
  and disabled rollout states.

### Changed

- Route generated APT and Scoop release publication through Simit's shared
  distribution helpers so consumers do not need project-local publisher scripts.

### Fixed

- Make generated flake-wiring CI checks read-only so fetching Simit cannot
  mutate its lock file in the consumer workspace.
- Emit native `owner/repository@sha` action references for GitHub workflows;
  keep URL-form references for Forgejo workflows.
- Keep the default `nix run` package independent of canix's managed compiler
  cache so public CI runners can build Simit without private cache transport.

## [0.17.7] - 2026-07-11

### Fixed

- Restrict Codeberg release uploads to configured release artifacts and
  checksum/signature files instead of every file in `release/`.

## [0.17.6] - 2026-07-10

### Fixed

- Apply the list-only package gate to multi-job workflows created by
  per-step runner routing.

## [0.17.5] - 2026-07-10

### Fixed

- Keep generated CI package checks list-only so unpublished workspace
  dependencies do not block first publication.

## [0.17.4] - 2026-07-10

### Fixed

- Use executable version checks for cargo-audit and cargo-deny in Nix
  workflows instead of passing the shell builtin `command` to `nix develop`.

## [0.17.3] - 2026-07-10

### Fixed

- Accept both Cargo `#` and `@` package-id delimiters while filtering Nix
  development-shell hook output from publish workflow version checks.

## [0.17.2] - 2026-07-10

### Fixed

- Keep Codeberg/Gitea-compatible dispatch triggers scalar instead of emitting
  unsupported nested workflow inputs.
- Filter Nix development-shell hook output when extracting a package version
  from `cargo pkgid` in publish workflows, accepting both Cargo's `#` and
  `@` package-id delimiters.

## [0.17.1] - 2026-07-10

### Fixed

- Keep generated release workflows out of CI drift detection so projects with
  both managed CI and release automation report a managed CI status.

## [0.17.0] - 2026-07-10

### Added

- Add persisted per-step runner routing and multi-job Forgejo CI rendering,
  including crate publishing and required environment handling.
- Add enforced tag-driven release publisher workflows with cross-platform
  artifact and Windows package support.
- Add regression fixtures for flake-provided `simitConfig` and Sorrel-like
  `simit.toml` configurations.

### Changed

- Extend `init release` to render comprehensive publisher workflows and make
  configured publisher enforcement explicit.

## [0.16.1] - 2026-05-30

### Fixed

- Fix `simit init ci --check` so generated but project-owned release workflows
  are ignored instead of being misreported as CI drift.

## [0.16.0] - 2026-05-30

### Added

- Document and integration-test multichannel distribution packaging commands,
  including `simit init aur|copr|apt|release` and
  `simit dist aur|copr|apt render` flows.

### Changed

- `simit init flake` now defaults to hooks-only ownership of
  `nix/pre-commit.nix`; use `--scope full` or `[flake].scope = "full"` to keep
  simit managing `flake.nix` and formatter wiring. Existing full-scope adopters
  are preserved until they opt into hooks-only.

### Fixed

- `simit init flake --check --diff` now explains pre-commit hook removals when
  a generated hook has a known CI-side replacement.

## [0.15.4] - 2026-05-25

### Changed

- `simit projects scan` now reports registry entries whose paths no longer
  exist on disk, including broken symlinks (use `--prune` to remove).
- `simit projects list` flags missing-path entries in its attention footer
  alongside `drift` / `conflicted` features, while filtering ephemeral `/tmp/*`
  paths from that footer.

### Fixed

- `simit hooks install` warns when a repo-local `core.hooksPath` shadows a
  friendly system dispatcher. Pass `--fix` to unset the rogue value
  automatically.
- Make `tests/projects.rs::list_filters_tmp_missing_paths_from_attention_footer`
  independent of `$TMPDIR`, so the test suite passes inside the Nix build
  sandbox where `$TMPDIR` is the build-local directory rather than `/tmp`.

## [0.15.3] - 2026-05-25

### Added

- Add `simit hooks install` with `--check` and `--diff` modes for installing
  project pre-commit hook wrappers even when `core.hooksPath` is set.
- Surface drift and hook-conflict project registry issues in
  `simit projects list`/`show`, with `--no-issues` available for scripts that
  need the old human table shape.

### Fixed

- Detect dispatcher-bypassing hook paths as `hooks: conflicted`, and require
  all managed hook wrappers before reporting `hooks: installed`.

## [0.15.2] - 2026-05-24

### Fixed

- Scope generated crates.io publish workflow tag/version validation to the
  package being published, avoiding wrong-version checks in Cargo workspaces
  whose members have diverging versions.

## [0.15.1] - 2026-05-24

### Fixed

- Fix generated crates.io publish workflows so Cargo workspace metadata version
  extraction selects a single package version instead of failing tag validation
  with concatenated workspace versions.

## [0.15.0] - 2026-05-24

### Added

- Add Omnix `om ci` support for Nix-runtime generated CI through
  `--with-om-ci` and `[ci].om_ci`, augment mode through `--om-ci-augment` and
  `[ci].om_ci_augment`, and Omnix flakeref pinning through `--omnix-ref`,
  `[ci].omnix_ref`, and user config `[ci.tools.omnix].ref`.
- Document the default Cargo-runtime CI cache layout generated by `simit init
ci`, including the `~/.cargo/bin` key, rust-cache settings, and why Nix
  workflows skip the Cargo cache steps.

## [0.14.1] - 2026-05-24

### Fixed

- Skip crates.io publishing for already-visible package versions in generated
  publish workflows before requiring a registry token.

## [0.14.0] - 2026-05-23

### Added

- Support project-configured custom flake validation for repositories that keep project-owned flake outputs.
- Render project-configured CI setup steps, environment variables, and required-secret documentation into generated workflows.

## [0.13.0] - 2026-05-23

### Added

- Add `simit projects list|show|scan|forget|prune|clear-state` for inspecting and
  maintaining the per-user project registry.
- Add `simit projects discover <ROOT>` to walk a filesystem subtree, find Cargo
  workspaces with at least one simit feature in use, and register them in the
  per-user project registry. Supports `--dry-run`, `--json`, `--max-depth`,
  `--skip`, `--include-empty`, and `--follow-symlinks`.
- Mark `simit projects discover --include-empty` entries with no detected simit
  features in human output instead of treating generic project files as simit
  usage.
- Add `simit projects clear-state` to clear all per-user project registry state,
  with `--dry-run` support for previewing the target registry path.
- Keep default `simit projects discover` human output compact by summarizing
  skipped non-simit projects instead of listing every skipped path.

### Changed

- Breaking CLI restructure: grouped the former top-level `simit init-ci`,
  `simit init-flake`, `simit init-homebrew-tap`, `simit init-chocolatey`, and
  `simit init-scoop-bucket` commands under `simit init ...`, and grouped the
  former top-level `simit homebrew`, `simit chocolatey`, and `simit scoop`
  package-manager commands under `simit dist ...`.

## [0.12.0] - 2026-05-22

### Added

- Generate an MSRV pre-push hook from workspace rust-version metadata in init-flake output.
- Allow `simit init-flake --check` to accept equivalent generated treefmt and pre-commit hook wiring instead of requiring byte-for-byte generated files.

## [0.11.0] - 2026-05-22

### Added

- Add `simit release trust status|init|check` for managing the maintainer
  OpenPGP trust root used by generated publish workflows.
- `simit init-ci` now discovers the release signing key and generates
  `keys/maintainers.gpg` instead of requiring maintainers to export it by hand.

## [0.10.0] - 2026-05-22

### Added

- Generated release workflows now verify GPG-signed tags against
  `keys/maintainers.gpg`, sign `SHA256SUMS.txt` with minisign, and emit
  cosign/SLSA provenance bundles for release archives.
- Document release integrity trust roots, required signing secrets, and
  consumer verification commands.

### Fixed

- Lower simit's own Rust requirement back to 1.85 so a local simit checkout can
  run inside projects using the generated Rust 1.85 release workflows.

## [0.9.0] - 2026-05-22

### Added

- `simit release sync-up` for rerunning a failed tag-triggered release from a
  fixed commit.
- Simit project config can now live in `simit.toml`, Cargo metadata, or flake
  `outputs.simitConfig`, with a Nix helper exposed as `lib.mkSimitConfig`.

### Changed

- `simit init-flake` now renders `rustfmt` with the Rust edition declared by
  workspace packages instead of hard-coding edition 2021.
- `simit init-flake --check` accepts custom `harbor-rs` flakes that preserve
  the generated pre-commit hook wiring through equivalent local bindings.
- Document and regression-test that `simit init-ci --check` enforces the
  generated tag-triggered crates.io publish workflow.

## [0.8.0] - 2026-05-22

### Added

- Chocolatey package publishing via simit chocolatey and init-ci --with-chocolatey
- Scoop bucket publishing via simit scoop and init-ci --with-scoop

### Changed

- release-artifacts.yaml now uses a Linux + Windows matrix when Windows packagers are enabled

## [0.7.0] - 2026-05-21

### Added

- `simit changelog` subcommand for managing keep-a-changelog `CHANGELOG.md`
  files (init / add / release / check / show), with optional auto-promotion
  from `simit release`.
- Add Forgejo/Nix Homebrew tap publishing to init-ci.
- Add optional `simit.toml` project config with a `[homebrew]` section for
  upcoming Homebrew subcommands.
- Add `simit init-homebrew-tap` for bootstrapping Homebrew tap repos with a
  checked-in `Formula/<name>.rb` skeleton.
- Add `simit homebrew render` and `simit homebrew bump` for native Homebrew
  formula rendering, sha256 computation, and optional tap commits/pushes
  without shelling out to `harbor-rs`.

## [0.3.1] - 2026-05-11

- Adjust generated Rust CI defaults

## [0.3.0] - 2026-05-09

- Reorg and release to crates.io

## [0.2.0] - 2026-05-09

- Prepare the first public crates.io release.

[Unreleased]: https://github.com/caniko/simit/compare/0.21.0...HEAD
[0.21.0]: https://github.com/caniko/simit/compare/0.20.0...0.21.0
[0.20.0]: https://github.com/caniko/simit/compare/0.19.0...0.20.0
[0.19.0]: https://github.com/caniko/simit/compare/0.18.1...0.19.0
[0.18.1]: https://github.com/caniko/simit/compare/0.18.0...0.18.1
[0.18.0]: https://github.com/caniko/simit/compare/0.17.15...0.18.0
[0.17.15]: https://github.com/caniko/simit/compare/0.17.14...0.17.15
[0.17.14]: https://github.com/caniko/simit/compare/0.17.13...0.17.14
[0.17.13]: https://github.com/caniko/simit/compare/0.17.12...0.17.13
[0.17.12]: https://github.com/caniko/simit/compare/0.17.11...0.17.12
[0.17.11]: https://github.com/caniko/simit/compare/0.17.10...0.17.11
[0.17.10]: https://github.com/caniko/simit/compare/0.17.9...0.17.10
[0.17.9]: https://github.com/caniko/simit/compare/0.17.8...0.17.9
[0.17.8]: https://github.com/caniko/simit/compare/0.17.7...0.17.8
[0.17.7]: https://github.com/caniko/simit/compare/0.17.6...0.17.7
[0.17.6]: https://github.com/caniko/simit/compare/0.17.5...0.17.6
[0.17.5]: https://github.com/caniko/simit/compare/0.17.4...0.17.5
[0.17.4]: https://github.com/caniko/simit/compare/0.17.3...0.17.4
[0.17.3]: https://github.com/caniko/simit/compare/0.17.2...0.17.3
[0.17.2]: https://github.com/caniko/simit/compare/0.17.1...0.17.2
[0.17.1]: https://github.com/caniko/simit/compare/0.17.0...0.17.1
[0.17.0]: https://github.com/caniko/simit/compare/0.16.1...0.17.0
[0.16.1]: https://github.com/caniko/simit/compare/0.16.0...0.16.1
[0.16.0]: https://github.com/caniko/simit/compare/0.15.4...0.16.0
[0.15.3]: https://github.com/caniko/simit/compare/0.15.2...0.15.3
[0.15.2]: https://github.com/caniko/simit/compare/0.15.1...0.15.2
[0.15.1]: https://github.com/caniko/simit/compare/0.15.0...0.15.1
[0.15.0]: https://github.com/caniko/simit/compare/0.14.1...0.15.0
[0.14.1]: https://github.com/caniko/simit/compare/0.14.0...0.14.1
[0.14.0]: https://github.com/caniko/simit/compare/0.13.0...0.14.0
[0.13.0]: https://github.com/caniko/simit/compare/0.12.0...0.13.0
[0.12.0]: https://github.com/caniko/simit/compare/0.11.0...0.12.0
[0.11.0]: https://github.com/caniko/simit/compare/0.10.0...0.11.0
[0.10.0]: https://github.com/caniko/simit/compare/0.9.0...0.10.0
[0.9.0]: https://github.com/caniko/simit/compare/0.8.0...0.9.0
[0.8.0]: https://github.com/caniko/simit/compare/0.7.0...0.8.0
[0.7.0]: https://github.com/caniko/simit/compare/0.3.1...0.7.0
