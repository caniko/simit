# Release Integrity

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
