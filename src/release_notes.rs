//! The same deterministic Git-notes script is used by CI and local verification.
use crate::release_identity::TagPrefix;

pub fn git_notes_script(prefix: TagPrefix) -> String {
    scoped_git_notes_script(prefix.as_str(), &[])
}

/// Generate notes bounded by one package's tags and component-owned paths.
pub fn component_git_notes_script(namespace: &str, paths: &[String]) -> anyhow::Result<String> {
    crate::release_identity::ReleaseTag::for_package(namespace, semver::Version::new(0, 0, 0))?;
    for path in paths {
        crate::monorepo::validate_relative_path(path)?;
    }
    anyhow::ensure!(
        !paths.is_empty(),
        "component release notes require owned paths"
    );
    Ok(scoped_git_notes_script(&format!("{namespace}/v"), paths))
}

fn scoped_git_notes_script(prefix: &str, paths: &[String]) -> String {
    // Choose the closest reachable release tag, excluding tags on the current
    // commit. Restrict candidates to this project's conventional release tags.
    let script = r#"set -euo pipefail
git rev-parse --verify "refs/tags/$TAG^{commit}" >/dev/null
previous=''
best_distance=''
while IFS= read -r candidate; do
  [ "$candidate" != "$TAG" ] || continue
  printf '%s\n' "$candidate" | grep -Eq '^PREFIX[0-9]+\.[0-9]+\.[0-9]+(-(rc|beta|alpha)\.[0-9]+)?$' || continue
  distance=$(git rev-list --count "$candidate..$TAG")
  [ "$distance" -gt 0 ] || continue
  if [ -z "$best_distance" ] || [ "$distance" -lt "$best_distance" ]; then
    previous="$candidate"
    best_distance="$distance"
  fi
done < <(git for-each-ref --merged "refs/tags/$TAG" --format='%(refname:strip=2)' refs/tags)
if [ -n "$previous" ]; then range="$previous..$TAG"; else range="refs/tags/$TAG"; fi
git log --reverse --format='- %s (%h)' "$range" --
"#
    .replace("PREFIX", prefix);
    let pathspecs = paths
        .iter()
        .map(|path| crate::commands::scaffold::shell_word(&format!(":(top,literal){path}")))
        .collect::<Vec<_>>()
        .join(" ");
    if pathspecs.is_empty() {
        script
    } else {
        format!("{} {pathspecs}\n", script.trim_end())
    }
}
