# Submit the manifests in "$dir" to microsoft/winget-pkgs through the GitHub
# REST API as the WINGET_PAT user. Needs curl and jq.
# Inputs: ID MANIFEST_DIR VERSION ZIP_URL WINGET_PAT, state (active-required when
# the package already exists upstream) and dir (the generated manifest files).
UP=microsoft/winget-pkgs
umask 077
# The token reaches curl through this 0600 header file, never through argv.
printf 'Authorization: Bearer %s\n' "$WINGET_PAT" > "$tmpdir/auth"
printf '{}' > "$tmpdir/empty.json"
code=; last=
# api METHOD PATH [BODYFILE]: body lands in $tmpdir/resp, status in $code, non-2xx returns 1.
api() {
  local a=(-sS --retry 3 -o "$tmpdir/resp" -w '%{http_code}' -X "$1"
    -H "@$tmpdir/auth" -H 'Accept: application/vnd.github+json'
    -H 'X-GitHub-Api-Version: 2022-11-28' -H 'Content-Type: application/json')
  [ -z "${3:-}" ] || a+=(--data-binary "@$3")
  last="$1 $2"
  code="$(curl "${a[@]}" "https://api.github.com$2")" || code=000
  case "$code" in 2??) ;; *) return 1 ;; esac
}
fail() { echo "::error::winget: $last -> HTTP $code: $(jq -r '.message // empty' "$tmpdir/resp" 2>/dev/null)" >&2; exit 1; }
must() { api "$@" || fail; }
val() { jq -r "$1" "$tmpdir/resp"; }

# Nothing to do when this version is already upstream.
api GET "/repos/$UP/contents/$MANIFEST_DIR/$VERSION" || true
case "$code" in
  200) echo "winget: $ID $VERSION is already published in $UP"; exit 0 ;;
  404) ;;
  *) fail ;;
esac
# New package or new version: the package directory decides.
if [ -z "$state" ]; then
  api GET "/repos/$UP/contents/$MANIFEST_DIR" || true
  case "$code" in 200) state=active-required ;; 404) state=inactive-soft ;; *) fail ;; esac
fi
if [ "$state" = active-required ]; then kind='New version'; else kind='New package'; fi
title="$kind: $ID version $VERSION"

must GET /user; login="$(val .login)"
must GET "/repos/$UP"; base="$(val .default_branch)"
branch="$(printf '%s-%s' "$ID" "$VERSION" | tr -c 'A-Za-z0-9._-' '-')"
# A PR that is still open is a success. When its head is this branch on the
# token user's own fork, the regenerated manifests are moved onto it below.
must GET "/repos/$UP/pulls?head=$login:$branch&state=open"
pr="$(val '.[0].html_url // empty')"
refresh=
if [ -n "$pr" ]; then
  if jq -e --arg b "$branch" --arg f "${login,,}/winget-pkgs" \
    '.[0] | .head.ref == $b and ((.head.repo.full_name // "") | ascii_downcase) == $f' "$tmpdir/resp" > /dev/null; then
    refresh=1
  else
    echo "winget: PR already open: $pr"; exit 0
  fi
fi

# POST /forks is idempotent; the fork can take a while to appear.
must POST "/repos/$UP/forks" "$tmpdir/empty.json"
fork="/repos/$login/winget-pkgs"
n=0
until api GET "$fork"; do
  n=$((n + 1))
  [ "$n" -lt 24 ] || { echo "::error::winget: fork $login/winget-pkgs did not appear within 2 minutes" >&2; exit 1; }
  sleep 5
done

must GET "/repos/$UP/git/ref/heads/$base"; head="$(val .object.sha)"
must GET "/repos/$UP/git/commits/$head"; base_tree="$(val .tree.sha)"
# A lagging fork may not have the upstream commit yet; sync it first.
api GET "$fork/git/commits/$head" || {
  jq -n --arg b "$base" '{branch: $b}' > "$tmpdir/b"
  must POST "$fork/merge-upstream" "$tmpdir/b"
}

# One commit holding the three manifests: blobs -> tree on the upstream tree -> commit -> ref.
entries='[]'
for f in "$dir"/*.yaml; do
  jq -n --rawfile c "$f" '{content: $c, encoding: "utf-8"}' > "$tmpdir/b"
  must POST "$fork/git/blobs" "$tmpdir/b"
  entries="$(jq -c --arg p "$MANIFEST_DIR/$VERSION/${f##*/}" --arg s "$(val .sha)" \
    '. + [{path: $p, mode: "100644", type: "blob", sha: $s}]' <<<"$entries")"
done
if [ -n "$refresh" ]; then
  # Identical blobs on the PR branch mean the PR already carries these manifests.
  want="$(jq -cS 'map({(.path | split("/") | last): .sha}) | add' <<<"$entries")"
  if api GET "$fork/contents/$MANIFEST_DIR/$VERSION?ref=$branch" \
    && [ "$(jq -cS 'map({(.name): .sha}) | add' "$tmpdir/resp")" = "$want" ]; then
    echo "winget: PR already open and up to date: $pr"; exit 0
  fi
fi
jq -n --arg b "$base_tree" --argjson e "$entries" '{base_tree: $b, tree: $e}' > "$tmpdir/b"
must POST "$fork/git/trees" "$tmpdir/b"; tree="$(val .sha)"
jq -n --arg m "$title" --arg t "$tree" --arg p "$head" '{message: $m, tree: $t, parents: [$p]}' > "$tmpdir/b"
must POST "$fork/git/commits" "$tmpdir/b"; commit="$(val .sha)"
if [ -n "$refresh" ]; then
  # Keep the PR: force-move its branch to the commit with the new manifests.
  jq -n --arg s "$commit" '{sha: $s, force: true}' > "$tmpdir/b"
  must PATCH "$fork/git/refs/heads/$branch" "$tmpdir/b"
  echo "winget: refreshed $pr with the regenerated manifests"; exit 0
fi
jq -n --arg r "refs/heads/$branch" --arg s "$commit" '{ref: $r, sha: $s}' > "$tmpdir/b"
api POST "$fork/git/refs" "$tmpdir/b" || {
  [ "$code" != 422 ] && fail
  # The branch survives from an earlier run whose PR was closed: move it.
  jq -n --arg s "$commit" '{sha: $s, force: true}' > "$tmpdir/b"
  must PATCH "$fork/git/refs/heads/$branch" "$tmpdir/b"
}

jq -n --arg t "$title" --arg h "$login:$branch" --arg b "$base" \
  --arg body "Automated submission of $ID $VERSION from $ZIP_URL, generated by simit." \
  '{title: $t, head: $h, base: $b, body: $body, maintainer_can_modify: true}' > "$tmpdir/b"
must POST "/repos/$UP/pulls" "$tmpdir/b"
echo "winget: opened $(val .html_url)"
