# Release Integrity

## Independent Publisher Admission

The `release_publisher` library binds a completed release event to its native
GitHub Actions run before an independently trusted default-branch publisher can
consume its artifacts. Policy supplies the exact repository, default branch and
source/publisher workflow identities. The publisher context comes from GitHub's
workflow context, and the run record must be fetched independently from GitHub.
Candidate artifacts cannot supply these trust roots.

Push binding rejects fork runs, unsuccessful runs, dispatches, noncanonical
semver head labels and event/provider drift. A native `head_branch` label cannot
distinguish `refs/tags/1.2.3` from `refs/heads/1.2.3`; `BoundReleasePush` retains
`source_head_label` and does not claim a tag identity. Independently trusted
tag-trigger provenance remains a required gate before publication.
Retries retain the original run ID while
recording its current attempt. Artifact verification binds opaque archive bytes
to the native artifact ID, run, repository, source SHA, expiry and SHA-256 digest
under a trusted byte limit. GitHub artifact metadata does not identify the
producing attempt; original-run archive reuse must not be described as fresh
attempt production.

These helpers establish intermediate push identity only. Signed-tag verification, safe
archive-member admission, archive-only publication, the protected environment's
ref restrictions and actual credential enrollment remain separate required
boundaries. The tag-controlled generator routes are not yet qualified for that
independent credential boundary. `Qualify publisher boundary` retains both helper
test results and the three generated credential-boundary regressions, including
failed candidates.

Generated release workflows treat signed tags and signed artifacts as part of
the release contract.

## Trust Roots

Commit these files in each project that uses generated release workflows:

```text
keys/maintainers.gpg
keys/minisign.pub
```

`keys/maintainers.gpg` is the OpenPGP public keyring used by CI to run
`git verify-tag` before publishing. `simit init ci` writes it automatically
from the configured release signing key. You can manage it directly:

```sh
simit release trust status
simit release trust init
simit release trust check
```

The key is discovered from `[release.signing].key`, `git config
user.signingkey`, or `--key`/`--maintainer-key`. If no exportable key is
available, simit blocks instead of generating an unsigned publish path.

Land `keys/maintainers.gpg` on the repository's default branch before releasing.
Generated publish workflows fetch that branch's keyring into an isolated GnuPG
home rather than trusting keys supplied by the release checkout. A missing
default-branch keyring fails publication. Key rotation must therefore land on
the default branch before a tag signed by the replacement key is released.

After verifying the signed tag, generated publish workflows require its peeled
commit to equal the checkout being published, before package metadata, project
`ci.extra_setup` commands, or publication execute. Checkout credentials are not
persisted; private verification fetches use command-scoped authentication.
GitHub crates.io publication uses two generated workflows. The signed semver
tag workflow builds credential-free archives and retains immutable per-attempt
artifacts. Its `trusted-publish-*.yaml` companion executes through `workflow_run`
from the independent default branch. A fresh credential-free job verifies and
compiles the packaged contents before a separate hosted upload job re-admits
the original source and artifact. Candidate configuration, compiler wrappers,
hooks and build scripts never execute with the registry credential.

Before enabling publication, the operator must move `CRATES_IO_API_TOKEN` into
the literal `simit-crates-io` environment, restrict that environment to the exact
protected default branch, and remove repository/organization publishing secrets
available to tag workflows. Set `vars.SIMIT_PUBLISHER_REVISION` to a qualified
immutable 40-hex Simit revision. Generation does not provision these boundaries;
an absent engine enrollment fails closed.

The publisher checks the native repository, workflow ID/path, original run and
attempt, provider artifact SHA-256, default-branch keyring and maintainer-signed
tag/source. It reconstructs package contents with isolated Cargo configuration
and `cargo package --no-verify`, then compares every member before API upload.
Reconstruction explicitly selects crates.io and uses a neutral filesystem-root
working directory with absolute manifest/target paths and private `HOME` /
`CARGO_HOME`. Cargo therefore does not inherit a candidate configuration or a
Nix build directory's ancestor vendor replacement, which would hide coordinated
unpublished workspace dependencies. This remains data-only packaging; candidate
build scripts and credential providers are not executed.
Member paths, permission modes and source/original-manifest bytes must match.
Generated `Cargo.toml` and `Cargo.lock` compare as typed TOML, preserving manifest
fields and the complete dependency/checksum graph while tolerating Cargo-version
differences in comments, layout and lock-format 3/4 markers. Changed dependency
identities, source bytes, extra files or permission bits are rejected.
Coordinated releases package their full release set together and publish in
dependency order, waiting for exact registry resolution before dependents run.
A moved tag, foreign run, changed archive or escaping manifest path is rejected.
Regenerate existing consumer workflows after selecting the qualified generator.

To retry publication, rerun the original signed-tag run. The earliest live
immutable artifact from that run is reused; an artifact name alone does not
prove attempt provenance. Forgejo retains its existing dispatch surface and
signed-tag/checkout validation.

An HTTP 200 for an existing crate version is not release acceptance. Publishers
require an unyanked exact crate/version record and a checksum matching the local
verified package archive, then independently resolve and fetch that exact
registry source/version/checksum. A matching upload can be resumed by rerunning
the original signed-tag run; a conflicting or unusable version fails closed.

`keys/minisign.pub` is the public half of the offline minisign key used to
sign `SHA256SUMS.txt`. Generate the keypair on the maintainer-controlled
machine and store the secret key outside the repository:

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
keyless certificate, the generated workflow falls back to `COSIGN_PRIVATE_KEY`
when that secret is configured; otherwise the release fails.

## Generated Artifacts

The artifact workflow expects the project build to place release assets in the
top-level `release/` directory. It then writes and publishes:

- `SHA256SUMS.txt`
- `SHA256SUMS.txt.minisig`
- `*.cosign.bundle` for tarballs, zip files, AppImages, and source RPMs
- `*.intoto.jsonl` and `*.intoto.bundle` SLSA v1 attestations for those assets

The SLSA predicate records the source commit, release workflow digest, optional
`flake.lock` digest, artifact name, and artifact SHA-256.

### Editor Marketplace Packages

JetBrains publication reads the signing task's `signedArchiveFile` property and
binds `verifyPluginSignature.inputArchiveFile` to that same archive. The verified
output is used even when the project overrides its name or directory; missing,
ambiguous, or unsigned selections fail before upload. See the
[JetBrains signing task contract](https://plugins.jetbrains.com/docs/intellij/tools-intellij-platform-gradle-plugin-tasks.html#signPlugin-signedArchiveFile).

VS Code Marketplace and Open VSX publishers pass every `release/*.vsix` package
through the plural `--packagePath` option, preserving universal and target-specific
packages. Qualification exercises publisher arguments and archive selection with
offline fixtures; it does not perform a live marketplace upload.

## Consumer Verification

Verify the signed checksum manifest first:

```sh
minisign -Vm SHA256SUMS.txt -p keys/minisign.pub
sha256sum -c SHA256SUMS.txt --ignore-missing
```

For keyless Sigstore bundles, constrain the expected identity and issuer for
the specific forge before trusting a release:

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
