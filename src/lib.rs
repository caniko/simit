//! Library support for `simit`, a semver-aware commit and release helper for
//! Rust projects.
//!
//! The public modules expose the same primitives used by the command-line
//! interface: Cargo metadata/version planning, changelog promotion, project
//! file generation, release preflight checks, and rendering for generated CI,
//! flake, and Homebrew files.

/// Cargo metadata parsing and version bump planning.
pub mod cargo;
/// Keep a Changelog file helpers.
pub mod changelog;
/// Shared CI option resolution for generation and drift detection.
pub mod ci_resolution;
/// Command-line argument definitions.
pub mod cli;
/// Command implementations used by the binary.
pub mod commands;
/// Project configuration loaded from supported simit project config sources.
pub mod config;
/// Git preflight, staging, commit, and tag helpers.
pub mod git;
/// Project language detection and generated-file management.
pub mod project;
/// README badge generation and project upgrade support.
pub mod readme_badges;
/// Per-user registry of projects simit has acted on.
pub mod registry;
/// Release maintainer trust-root discovery and validation.
pub mod release_trust;
/// Renderers for generated support files.
pub mod render;
/// SHA-256 helpers for release artifacts.
pub mod sha256;
/// User-scoped configuration for local infrastructure defaults.
pub mod user_config;
