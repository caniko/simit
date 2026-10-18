//! Library support for `simit`, a semver-aware commit and release helper for
//! Rust projects.
//!
//! The public modules expose the same primitives used by the command-line
//! interface: Cargo metadata/version planning, changelog promotion, project
//! file generation, release preflight checks, and rendering for generated CI,
//! flake, and Homebrew files.

/// Shared bounded execution for CI and runtime admission.
pub mod bounded_process;
/// Caller-owned cancellation shared by CI and runtime execution.
pub mod cancellation;
/// Cargo metadata parsing and version bump planning.
pub mod cargo;
/// Keep a Changelog file helpers.
pub mod changelog;
/// Shared CI option resolution for generation and drift detection.
pub mod ci_resolution;
/// Versioned grouped CI execution, separate from rendering and qualification authority.
pub mod ci_runtime;
/// Command-line argument definitions.
pub mod cli;
/// Command implementations used by the binary.
pub mod commands;
/// Project configuration loaded from supported simit project config sources.
pub mod config;
/// Git preflight, staging, commit, and tag helpers.
pub mod git;
/// Declarative mixed-language repository components and qualification plans.
pub mod monorepo;
/// Shared packaging validation + release-version helpers.
pub mod packaging_common;
/// Shared Codeberg Pages workflow inference.
pub mod pages_infer;
/// Project language detection and generated-file management.
pub mod project;
/// Python uv project metadata discovery.
pub mod python;
/// README badge generation and project upgrade support.
pub mod readme_badges;
/// Per-user registry of projects simit has acted on.
pub mod registry;
/// Canonical release tags and their semantic package versions.
pub mod release_identity;
/// Deterministic release notes from the exact tagged Git history.
pub mod release_notes;
/// Independent default-branch publisher event and provider identity binding.
pub mod release_publisher;
/// Release maintainer trust-root discovery and validation.
pub mod release_trust;
/// Renderers for generated support files.
pub mod render;
/// Exact-revision repository review, closure promotion, and retrieval.
pub mod review;
/// Bounded runtime readiness observations with explicit retry policy.
pub mod runtime_admission;
/// SHA-256 helpers for release artifacts.
pub mod sha256;
/// User-scoped configuration for local infrastructure defaults.
pub mod user_config;
