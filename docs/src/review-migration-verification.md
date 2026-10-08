# Review migration verification

The generic engine was imported from controller commit
`f7bad6b95787f752cedbf50e09c5bf4ae0f39a07`, retaining the upstream fork license.
The controller keeps its existing history and deployment identity; it consumes
Simit through an exact, content-hashed source input rather than a duplicate
implementation.

Local verification covers the full Simit suite through `simit test --git-fixtures`,
the 19 imported contract/CLI regressions, standalone `simit review` validation,
engine/tool lock tamper rejection, controller/client generation and drift, and
Clippy with `-D warnings`. All four generated v1 schemas match the controller
schemas. Real Nix builds and closure operations run only in GitHub CI.

CI retains the existing Simit gates and adds Linux/Windows Rust 1.85 checks,
packaged runtime-asset checks, and pinned controller-package/adapter checks.
The controller CI covers conventional, external and failing checks, complete
closure transfer, real cache misses, unsigned/wrong-key rejection, and signed
exact-path copy under consumer `require-sigs = false`. The deployment lock
receipt is generated in CI from the preserved reviewed tool lock, not by local
Nix evaluation.

## Review notes

An independent review checked engine pin binding, static generation, target-job
credential separation, and promotion policy. Its proposed reversal of the OIDC
claims was rejected against [GitHub's reusable-workflow OIDC documentation](https://docs.github.com/en/actions/security-for-github-actions/security-hardening-your-deployments/using-openid-connect-with-reusable-workflows):
`job_workflow_ref` describes the **called** reusable workflow; standard claims
describe its caller. The controller must retain the called-workflow identity.

CI caught copied fixture formatting, Windows package-list path separators,
temporary flake Git filtering, and supplementary-workflow ownership. These were
fixed without dropping their checks. The main Crane source filter retains the
embedded review assets, while the lean controller package includes its own
runtime tools and adapter.

These are implementation and CI evidence, not production acceptance. Publication
remains disabled, approved public-cache keys remain unset, and descriptors remain
`ready: false`. Human review, manual default-branch merges, live dispatch,
digest-approved cache publication, and fresh-store retrieval are distinct
activation gates. OpenPencil integration remains a separate unresolved request.
