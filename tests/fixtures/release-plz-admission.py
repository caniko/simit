"""Execute release selection and the Cargo boundary against real Git histories."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

helper = Path(sys.argv[1]).resolve()
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("admission", helper)
admission = importlib.util.module_from_spec(spec)
spec.loader.exec_module(admission)


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


with tempfile.TemporaryDirectory() as directory:
    os.chdir(directory)
    git("init", "-b", "trunk")
    tree = subprocess.check_output(["git", "mktree"], input="", text=True).strip()

    def commit(message, *parents):
        # Disposable object fixtures need no operator identity, hooks or signer.
        text = f"tree {tree}\n" + "".join(f"parent {parent}\n" for parent in parents)
        text += "author Fixture <fixture@example.invalid> 1700000000 +0000\n"
        text += "committer Fixture <fixture@example.invalid> 1700000000 +0000\n\n"
        return subprocess.check_output(
            ["git", "hash-object", "-t", "commit", "--stdin", "-w"],
            input=text + message + "\n",
            text=True,
        ).strip()

    base = commit("initial")
    prepared = commit("release", base)
    merged = commit("merge", base, prepared)
    git("checkout", "--detach", merged)
    os.environ["GITHUB_REPOSITORY"] = "owner/repo"
    pr = {
        "number": 17,
        "head": {"ref": "release-plz-test"},
        "merged_at": "2026-10-19T00:00:00Z",
        "base": {"repo": {"full_name": "owner/repo"}},
    }
    commits = lambda number: [{"sha": prepared}]
    assert admission.release_sha(merged, [pr], commits) == prepared
    assert admission.release_sha(merged, [], commits) == ""
    try:
        admission.release_sha(merged, [{**pr, "merged_at": None}], commits)
        raise AssertionError("accepted an unmerged release PR")
    except RuntimeError:
        pass

    def guard(expected, *args):
        return subprocess.run(
            [sys.executable, str(helper), "cargo", *args],
            env={
                **os.environ,
                "SIMIT_EXPECTED_RELEASE_SHA": expected,
                "SIMIT_REAL_CARGO": shutil.which("true"),
            },
            capture_output=True,
        )

    # Qualification of the branch tip is insufficient for a merge release.
    assert guard(prepared, "publish").returncode != 0
    assert guard(prepared, "publish", "--dry-run").returncode != 0
    assert guard(prepared, "metadata").returncode == 0
    git("checkout", "--detach", prepared)
    assert guard(prepared, "publish").returncode == 0
    assert guard("", "publish").returncode != 0

    squashed = commit("squash", base)
    git("checkout", "--detach", squashed)
    assert admission.release_sha(squashed, [pr], commits) == squashed
    assert guard(squashed, "publish").returncode == 0
    assert guard(prepared, "publish").returncode != 0
    # An API result changing between admission and publication fails closed.
    os.environ["GITHUB_SHA"] = squashed
    os.environ["GITHUB_OUTPUT"] = str(Path(directory) / "output")
    os.environ["SIMIT_EXPECTED_RELEASE_SHA"] = prepared
    admission.api = lambda path: [pr] if "/commits/" in path else commits(17)
    try:
        admission.select()
        raise AssertionError("accepted a changed release selection")
    except RuntimeError:
        pass

    # The workflow must sign with a key authorized by the committed trust root.
    home = Path(directory) / "gnupg"
    home.mkdir(mode=0o700)
    os.environ["GNUPGHOME"] = str(home)

    def key(home, uid):
        subprocess.run(
            [
                "gpg", "--homedir", str(home), "--batch", "--pinentry-mode", "loopback",
                "--passphrase", "", "--quick-generate-key", uid, "ed25519", "sign", "0",
            ],
            check=True,
            capture_output=True,
        )
        return subprocess.check_output(
            ["gpg", "--homedir", str(home), "--armor", "--export"], text=True
        )

    trust = Path(directory) / "trust.asc"
    trust.write_text(key(home, "Release Fixture <fixture@example.invalid>"))
    admission.signing_identity(str(trust))
    assert git("config", "user.email") == "fixture@example.invalid"
    git("config", "tag.gpgSign", "true")
    git("tag", "v0.2.0", "-m", "release")
    git("verify-tag", "v0.2.0")
    other_home = Path(directory) / "other-gnupg"
    other_home.mkdir(mode=0o700)
    trust.write_text(key(other_home, "Other Fixture <other@example.invalid>"))
    try:
        admission.signing_identity(str(trust))
        raise AssertionError("accepted a private key outside the trust root")
    except RuntimeError:
        pass
    for home in [home, other_home]:
        subprocess.run(
            ["gpgconf", "--homedir", str(home), "--kill", "gpg-agent"], check=True
        )

print("release-plz merge, squash, publication and authorized tag-signing checks passed")
