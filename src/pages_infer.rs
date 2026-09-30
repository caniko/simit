//! Shared Codeberg Pages workflow inference.
//!
//! Canonical home for the `infer_pages_*` + `shell_unquote` helpers previously
//! copy-pasted across `commands::init_ci`, `registry`, and `commands::upgrade`.
//! Canonical behavior: robust `branches:` parser (inline `["x"]` + `- x` list)
//! from `init_ci`, and `'\''` unescaping from `registry` (matches what the
//! renderers emit via `value.replace('\'', "'\\''")`).

pub fn infer_pages_repo(content: &str) -> Option<String> {
    let marker = "@codeberg.org/";
    let line = content.lines().find(|line| line.contains(marker))?;
    let repo_start = line.find(marker)? + marker.len();
    let repo_tail = &line[repo_start..];
    let repo_end = repo_tail.find(".git").unwrap_or(repo_tail.len());
    Some(repo_tail[..repo_end].trim_matches('"').to_owned())
}

pub fn infer_pages_token_secret(content: &str) -> Option<String> {
    let marker = "CODEBERG_TOKEN: ${{ secrets.";
    let line = content.lines().find(|line| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(" }}")?;
    Some(tail[..end].to_owned())
}

pub fn infer_pages_canonical_domain(content: &str) -> Option<String> {
    let marker = "grep -qx ";
    let suffix = " result-pages-site/.domains";
    let line = content
        .lines()
        .find(|line| line.contains(marker) && line.contains(suffix))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(suffix)?;
    Some(shell_unquote(tail[..end].trim()))
}

pub fn infer_pages_site_output(content: &str) -> Option<String> {
    let marker = "nix build ";
    let suffix = " --out-link result-pages-site";
    let line = content
        .lines()
        .find(|line| line.contains(marker) && line.contains(suffix))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(suffix)?;
    let output = tail[..end].trim();
    // Accept workflows generated before the output-link fix during upgrades.
    Some(shell_unquote(
        output.strip_suffix(" --no-link").unwrap_or(output),
    ))
}

pub fn infer_pages_source_branch(content: &str) -> Option<String> {
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim_start().starts_with("branches:") {
            continue;
        }
        let trimmed = line.trim();
        if let Some(inline) = trimmed
            .strip_prefix("branches: [")
            .and_then(|value| value.strip_suffix(']'))
        {
            return Some(inline.trim_matches('"').to_owned());
        }
        while let Some(next) = lines.peek() {
            let trimmed = next.trim();
            if let Some(branch) = trimmed.strip_prefix("- ") {
                return Some(branch.trim_matches('"').to_owned());
            }
            if !next.starts_with(' ') {
                break;
            }
            lines.next();
        }
    }
    None
}

pub fn infer_pages_deploy_app(content: &str) -> Option<String> {
    let marker = "DEPLOY_REMOTE=pages-origin nix run ";
    let line = content.lines().find(|line| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    Some(line[start..].trim().to_owned())
}

pub fn shell_unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("'\\''", "'");
    }
    value.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_branch_parses_inline_and_list_forms() {
        assert_eq!(
            infer_pages_source_branch("on:\n  push:\n    branches: [\"trunk\"]\n"),
            Some("trunk".to_owned())
        );
        assert_eq!(
            infer_pages_source_branch("on:\n  push:\n    branches:\n      - trunk\n"),
            Some("trunk".to_owned())
        );
        assert_eq!(infer_pages_source_branch("on: push\n"), None);
    }

    #[test]
    fn shell_unquote_reverses_renderer_escaping() {
        assert_eq!(shell_unquote("'a'\\''b'"), "a'b");
        assert_eq!(shell_unquote("plain"), "plain");
        assert_eq!(shell_unquote("  '.#site'  "), ".#site");
    }

    #[test]
    fn markers_extract_expected_fields() {
        let content = "url ssh://git@codeberg.org/owner/repo.git\n\
             CODEBERG_TOKEN: ${{ secrets.MY_TOKEN }}\n\
             run: grep -qx 'example.com' result-pages-site/.domains\n\
             run: nix build '.#site' --no-link --out-link result-pages-site\n\
             run: DEPLOY_REMOTE=pages-origin nix run .#deploy-pages\n";
        assert_eq!(infer_pages_repo(content), Some("owner/repo".to_owned()));
        assert_eq!(
            infer_pages_token_secret(content),
            Some("MY_TOKEN".to_owned())
        );
        assert_eq!(
            infer_pages_canonical_domain(content),
            Some("example.com".to_owned())
        );
        assert_eq!(infer_pages_site_output(content), Some(".#site".to_owned()));
        assert_eq!(
            infer_pages_deploy_app(content),
            Some(".#deploy-pages".to_owned())
        );
    }
}
