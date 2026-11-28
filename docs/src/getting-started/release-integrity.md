# Release Integrity

## Independent Publisher Admission

Before an independently trusted default-branch publisher consumes artifacts,
`release_publisher` binds the completed release event to its native GitHub
Actions run. Policy supplies exact repository, default-branch and
source/publisher workflow identities. Take the publisher context from GitHub's
workflow context and fetch the run record independently from GitHub; candidate
artifacts must supply neither trust root.

Push binding rejects fork runs, unsuccessful runs, dispatches, noncanonical
semver head labels and event/provider drift. A native `head_branch` label cannot
distinguish `refs/tags/1.2.3` from `refs/heads/1.2.3`; `BoundReleasePush` retains
`source_head_label` and does not claim a tag identity. Independently trusted
tag-trigger provenance is required before publication.
Retries keep the original run ID and record the current attempt.
Under a trusted byte limit, verification binds opaque archive bytes to the
native artifact ID, run, repository, source SHA, expiry and SHA-256 digest.
GitHub artifact metadata cannot identify the producing attempt. Reusing an
original-run archive does not identify the producing attempt; never report it as
fresh attempt production.

These helpers establish only intermediate push identity. Signed-tag
verification, safe archive-member admission, archive-only publication,
protected-environment ref restrictions and credential enrollment remain separate
required boundaries. Tag-controlled generator routes do not yet qualify for the
independent credential boundary. `Qualify publisher boundary` retains helper
test results and the three generated credential-boundary regressions, including
failed candidates.

Generated release workflows require signed tags and signed artifacts.

## Trust Roots

Commit these files in each project that uses generated release workflows:

```text
keys/maintainers.gpg
keys/minisign.pub
```

CI uses the OpenPGP public keyring `keys/maintainers.gpg` to run
`git verify-tag` before publishing. `simit init ci` writes it from the configured
release signing key. To manage it directly:

```sh
simit release trust status
simit release trust init
simit release trust check
```

Simit looks for the key in `[release.signing].key`, `git config
user.signingkey`, or `--key`/`--maintainer-key`. Without an exportable key,
simit blocks rather than generating an unsigned publish path.

Land `keys/maintainers.gpg` on the repository's default branch before releasing.
Publish workflows fetch that keyring into an isolated GnuPG home. They never
trust checkout keys and fail without the default-branch keyring. Land key
rotations there before releasing tags signed with the replacement key.

Publish workflows verify the signed tag and require its peeled commit to equal
the commit being published before reading package metadata, running
`ci.extra_setup` commands or publishing.
They never persist checkout credentials; private verification fetches use
command-scoped authentication.
GitHub crates.io publication uses two generated workflows. The signed semver
tag workflow builds credential-free archives and retains immutable per-attempt
artifacts. Its `trusted-publish-*.yaml` companion runs via `workflow_run` from
the independent default branch. A fresh credential-free job verifies and
compiles the packaged contents before a separate hosted upload job re-admits
the original source and artifact. Candidate configuration, compiler wrappers,
hooks and build scripts never execute with the registry credential.
Tag archive builds and candidate verification always use fresh hosted
`ubuntu-24.04` runners, regardless of configured CI/release runner labels.
Jobs reapply the nonsecret build environment, required environment checks and
setup after source/archive admission and exact signed-source checkout. These
inputs affect only candidate setup, compilation and gates, not independent
engine bootstrap, admission or the separate upload job.

Before enabling publication, move `CRATES_IO_API_TOKEN` into the literal
`simit-crates-io` environment, restrict that environment to the exact
protected default branch, and remove repository/organization publishing secrets
available to tag workflows. Set `vars.SIMIT_PUBLISHER_REVISION` to a qualified
immutable 40-hex Simit revision. Generation does not provision these boundaries;
without engine enrollment, publication fails closed.

The publisher checks the native repository, workflow ID/path, original run and
attempt, provider artifact SHA-256, default-branch keyring and maintainer-signed
tag/source. It reconstructs package contents with isolated Cargo configuration
and `cargo package --no-verify`, then compares every member before API upload.
The full signed source workflow must match the independently enrolled workflow
as typed YAML, including permissions, runners, jobs and credential contexts.
Matching tag triggers alone do not authorize changes. Enroll workflow changes
independently on the default branch before release; comments and YAML key order
do not affect execution authority.
Reconstruction explicitly selects crates.io and uses a neutral filesystem-root
working directory with absolute manifest/target paths and private `HOME` / `CARGO_HOME`.
This isolates Cargo from candidate config and ancestor vendor replacements in
Nix build directories that would hide coordinated unpublished workspace
dependencies. Packaging is data-only: it runs no candidate build scripts or
credential providers.
Publisher validation skips free-form package/workspace metadata but requires
real package and dependency read paths to stay inside the signed checkout.
Nix-runtime publishers provision Nix only in credential-free verification,
before rerunning required source gates.
Member paths, permission modes and source/original-manifest bytes must match.
Comparing generated `Cargo.toml` and `Cargo.lock` as typed TOML preserves
manifest fields and the full dependency/checksum graph while allowing
Cargo-version differences in comments, layout and lock-format 3/4 markers.
The publisher rejects changes to dependency identities, source bytes or
permission bits, plus extra files, moved tags, foreign runs, changed archives
and escaping manifest paths. Coordinated releases package the full set together
and publish in dependency order, waiting for exact registry resolution before
dependents run.
Manifest validation follows the root Cargo workspace, member globs/exclusions
and local dependency graph; invalid unrelated test-fixture manifests do not
block release. On the credential-free runner, Nix projects verify the admitted
packaged crate in the original signed source's `nix develop` environment.
Isolated reconstruction uses Cargo's Git CLI transport and keeps only the fixed
public GitHub SSH-to-HTTPS rewrites. Dependency fetches receive no ambient Git
configuration, SSH agents or provider/registry credentials.
The signed normalized manifest must authorize crates.io publication: `publish =
false`, an empty list or a list excluding `crates-io` fails before upload.
The tag archive and candidate verification generators reject GitHub `secrets`
expressions, including in global CI environment, setup and required-gate bindings.
These inputs also reject `github.token`, whole-`github` serialization, wildcard
access and dynamic `github` indexing. Static non-token fields like
`github.ref_name` and `github['ref_name']` work. Quoted expression delimiters
cannot hide credential references.
Even direct renderer callers must use ASCII variable identifiers
(`[A-Za-z_][A-Za-z0-9_]*`) for required-environment names and candidate environment
keys. The generator rejects every invalid binding before writing files,
including names containing shell default expansions, command substitutions or
GitHub expressions.
Fixed generator-owned admission steps use command-local read-only provider
authentication for signed-source fetches. Build inputs cannot copy that token
but can use nonsecret build limits and native GitHub labels. Put authenticated
work in independently protected jobs.

Regenerate existing consumer workflows after selecting the qualified generator.

To retry publication, rerun the original signed-tag run. The retry reuses its
earliest live immutable artifact; a name alone does not prove attempt provenance.
Forgejo retains its dispatch surface and signed-tag/checkout validation.

An HTTP 200 for an existing version is not enough: publishers require the exact
unyanked crate/version and a checksum matching the verified local archive. They
then independently resolve and fetch that registry source/version/checksum.
Rerunning the original signed-tag run can resume a matching upload; a conflicting
or unusable version fails closed.
The publisher accepts Cargo's documented successful `warnings` response without
`ok` and rejects HTTP or explicit API errors. Lost, malformed or unsuccessful
replies trigger bounded, read-only reconciliation of the exact registry name,
version, checksum and yanked state, with no reupload in that invocation. Unknown
outcomes retain the original run/archive for later reconciliation. Every
reconciled upload must pass independent exact Cargo registry resolution;
dependents cannot publish until it passes.

The offline minisign key signs `SHA256SUMS.txt`; `keys/minisign.pub` holds its
public half. Generate the keypair on a maintainer-controlled machine and keep
the secret key outside the repository:

```sh
mkdir -p keys
minisign -G -p keys/minisign.pub -s minisign.sec
```

## Workflow Secrets

`simit init ci --with-artifacts` generates an artifact workflow that requires:

- `MINISIGN_SECRET_KEY`: contents of the password-protected minisign secret key.
- `MINISIGN_PASSWORD`: password for `MINISIGN_SECRET_KEY`.
- `COSIGN_PRIVATE_KEY`: optional fallback when keyless Sigstore OIDC is unavailable.
- `COSIGN_PASSWORD`: optional password for `COSIGN_PRIVATE_KEY`.

GitHub workflows request `id-token: write`. Forgejo workflows set
`enable-openid-connect: true`. If the platform cannot obtain a Fulcio/Rekor
keyless certificate, the workflow falls back to the `COSIGN_PRIVATE_KEY` secret
if configured; otherwise the release fails.

## Generated Artifacts

Place built release assets in the top-level `release/` directory. The artifact
workflow writes and publishes:

- `SHA256SUMS.txt`
- `SHA256SUMS.txt.minisig`
- `*.cosign.bundle` for tarballs, zip files, AppImages, and source RPMs
- `*.intoto.jsonl` and `*.intoto.bundle` SLSA v1 attestations for those assets

The SLSA predicate records the source commit, release workflow digest, optional
`flake.lock` digest, artifact name, and artifact SHA-256.

### Editor Marketplace Packages

The JetBrains publisher reads the signing task's `signedArchiveFile` property
and binds `verifyPluginSignature.inputArchiveFile` to the same archive. It uses
that verified output even if the project overrides its name or directory;
missing, ambiguous or unsigned selections fail before upload. See the
[JetBrains signing task contract](https://plugins.jetbrains.com/docs/intellij/tools-intellij-platform-gradle-plugin-tasks.html#signPlugin-signedArchiveFile).

VS Code Marketplace and Open VSX publishers pass every `release/*.vsix` to the
plural `--packagePath` option, preserving universal and target-specific packages.
Qualification tests publisher arguments and archive selection with offline
fixtures, not live marketplace uploads.

## Consumer Verification

Verify the signed checksum manifest first:

```sh
minisign -Vm SHA256SUMS.txt -p keys/minisign.pub
sha256sum -c SHA256SUMS.txt --ignore-missing
```

Before trusting the release, constrain the expected identity and issuer for the
releasing forge:

```sh
cosign verify-blob \
  --bundle foo-<version>-x86_64-linux.tar.gz.cosign.bundle \
  --certificate-identity-regexp '.*OWNER/REPO.*' \
  --certificate-oidc-issuer-regexp '.*' \
  foo-<version>-x86_64-linux.tar.gz

cosign verify-blob-attestation \
  --bundle foo-<version>-x86_64-linux.tar.gz.intoto.bundle \
  --type slsaprovenance1 \
  --certificate-identity-regexp '.*OWNER/REPO.*' \
  --certificate-oidc-issuer-regexp '.*' \
  foo-<version>-x86_64-linux.tar.gz
```
