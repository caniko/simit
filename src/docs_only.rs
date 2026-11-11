//! One definition of a documentation-only change.
//!
//! Root Markdown that no build reads is documentation. Nested Markdown can
//! feed documentation builds, and crates commonly `include_str!` their README,
//! so both keep full qualification. Monorepo planning and generated CI must
//! agree, so both derive from these constants.

/// Root Markdown files that builds and packages compile.
pub const DOCS_ONLY_EXCLUDED_PATHS: &[&str] = &["README.md"];

/// Extension of a documentation-only path, without the dot.
pub const DOCS_ONLY_EXTENSION: &str = "md";

/// Whether `path`, relative to the repository root, is a root Markdown file
/// whose change cannot affect a build.
pub fn is_docs_only_path(path: &str) -> bool {
    !path.contains('/')
        && path.len() > DOCS_ONLY_EXTENSION.len() + 1
        && path.ends_with(&format!(".{DOCS_ONLY_EXTENSION}"))
        && !DOCS_ONLY_EXCLUDED_PATHS.contains(&path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn root_markdown_is_documentation_but_nested_markdown_and_readmes_are_not() {
        for path in ["VISION.md", "NOTES.md", "CHANGELOG.md", "some notes.md"] {
            assert!(super::is_docs_only_path(path), "{path}");
        }
        for path in [
            "README.md",
            "docs/VISION.md",
            "crates/demo/NOTES.md",
            ".md",
            "VISION",
            "LICENSE",
            "src/main.rs",
            "",
        ] {
            assert!(!super::is_docs_only_path(path), "{path}");
        }
    }
}
