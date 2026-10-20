#!/usr/bin/env bash
# Hosted runner preparation only; production host policy belongs to its flake.
set -euo pipefail
test "${GITHUB_ACTIONS:-}" = true
bwrap=$(readlink -f "$(command -v bwrap)")
# Preserve the applet name: Nix's true can be a symlink to multicall coreutils.
true_program="$(readlink -f "$(dirname "$(type -P true)")")/true"
case "$bwrap" in /nix/store/*/bin/bwrap) ;; *) exit 1 ;; esac
case "$true_program" in /nix/store/*/bin/true) ;; *) exit 1 ;; esac
uname -a
cat /proc/self/attr/current
grep '^Cap' /proc/self/status
for setting in kernel.apparmor_restrict_unprivileged_userns user.max_user_namespaces; do
  sysctl "$setting" || true
done
probe() {
  env -i PATH="$PATH" bwrap --unshare-all --unshare-user --die-with-parent --new-session \
    --as-pid-1 --cap-drop ALL --disable-userns \
    --ro-bind /nix/store /nix/store --proc /proc --dev /dev --tmpfs /tmp \
    -- "$true_program"
}
started=$(date --iso-8601=seconds)
if probe; then
  echo 'Namespace startup succeeded without a host-policy allowance.'
  exit 0
fi
sudo journalctl -k --since "$started" --no-pager
# Ubuntu restricts unprofiled store executables, unlike its /usr/bin/bwrap.
# Admit only this immutable executable; leave the global restriction enabled.
test "$(sysctl -n kernel.apparmor_restrict_unprivileged_userns)" = 1
profile=$(mktemp)
trap 'rm -f "$profile"' EXIT
cat > "$profile" <<EOF
abi <abi/4.0>,
profile simit-updater-qualification "$bwrap" flags=(unconfined) {
  userns,
}
EOF
sudo apparmor_parser -r "$profile"
probe
echo 'Namespace startup succeeded with the executable-scoped userns allowance.'
