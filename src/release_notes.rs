//! The same deterministic Git-notes script is used by CI and local verification.
use crate::release_identity::TagPrefix;

pub fn git_notes_script(prefix: TagPrefix) -> String {
    // Choose the closest reachable release tag, excluding tags on the current
    // commit. Restrict candidates to this project's conventional release tags.
    r#"set -euo pipefail
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
    .replace("PREFIX", prefix.as_str())
}
