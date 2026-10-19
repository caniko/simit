"""Exercise data-only PR admission, signed delivery, and manual-edit protection."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("admission", Path(sys.argv[1]).resolve())
admission = importlib.util.module_from_spec(spec)
spec.loader.exec_module(admission)


def git(*args, **kwargs):
    return subprocess.check_output(["git", *args], text=True, **kwargs).strip()


with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    os.chdir(root)
    git("init", "-b", "trunk")
    gpg = root / "gnupg"
    gpg.mkdir(mode=0o700)
    os.environ["GNUPGHOME"] = str(gpg)
    subprocess.run(["gpg", "--batch", "--pinentry-mode", "loopback", "--passphrase", "", "--quick-generate-key", "Release Fixture <fixture@example.invalid>", "ed25519", "sign", "0"], check=True, capture_output=True)
    fingerprint = next(line.split(":")[9] for line in subprocess.check_output(["gpg", "--with-colons", "--list-secret-keys"], text=True).splitlines() if line.startswith("fpr:"))
    git("config", "user.name", "Release Fixture")
    git("config", "user.email", "fixture@example.invalid")
    git("config", "user.signingkey", fingerprint)
    (root / "keys").mkdir()
    (root / "keys/maintainers.gpg").write_bytes(subprocess.check_output(["gpg", "--export", fingerprint]))
    original = '[package]\nname="fixture"\nversion="0.1.0"\n'
    (root / "Cargo.toml").write_text(original)
    (root / ".github/workflows").mkdir(parents=True)
    (root / ".github/workflows/ci.yml").write_text("jobs: {}\n")
    (root / "old").mkdir()
    (root / "old/Cargo.lock").write_text("# lockfile\n")
    git("add", "Cargo.toml", ".github", "old", "keys")
    tree = git("write-tree")
    commit = f"tree {tree}\nauthor Fixture <fixture@example.invalid> 1700000000 +0000\ncommitter Fixture <fixture@example.invalid> 1700000000 +0000\n\ninitial\n"
    sha = git("hash-object", "-t", "commit", "--stdin", "-w", input=commit)
    git("update-ref", "refs/heads/trunk", sha)
    runtime = root / "runtime"
    runtime.mkdir()
    (runtime / "release-plz-update.log").write_text("fixture 0.1.0 -> 0.2.0\n")
    os.environ.update({
        "RUNNER_TEMP": str(runtime), "GITHUB_SHA": sha, "GITHUB_REPOSITORY": "owner/repo",
        "GITHUB_REF_NAME": "trunk", "GITHUB_ENV": str(runtime / "env"),
        "RELEASE_TRUST_ROOT": "keys/maintainers.gpg",
    })
    (root / "Cargo.toml").write_text(original.replace("0.1.0", "0.2.0"))
    (root / "CHANGELOG.md").write_text("# Changelog\n\n## 0.2.0\n\nPrepared by release-plz.\n")
    admission.prepare_pr()
    patch = (runtime / "release-plz-pr/update.patch").read_bytes()
    assert b"CHANGELOG.md" in patch, "new changelogs must cross the data boundary"
    git("reset", "--hard", sha)
    admission.admit_pr()
    os.environ["SIMIT_RELEASE_PR_TREE"] = (runtime / "env").read_text().strip().split("=", 1)[1]
    assert (root / "Cargo.toml").read_text() == original, "admission must only change the index"
    assert not (root / "CHANGELOG.md").exists()
    assert git("show", ":Cargo.toml").find("0.2.0") != -1
    remote = root / "remote.git"
    git("init", "--bare", str(remote))
    git("remote", "add", "origin", str(remote))
    os.environ["GIT_TOKEN"] = "fixture-write-token"
    original_check_output = subprocess.check_output
    remote_sha = sha

    def check_output(args, **kwargs):
        if args[:2] == ["gh", "api"]:
            return json.dumps({"object": {"sha": remote_sha}})
        return original_check_output(args, **kwargs)

    admission.subprocess.check_output = check_output
    # GitHub retains open PRs after their fork repository has been deleted.
    prs = [{"number": 2, "head": {"ref": "release-plz-deleted-fork", "repo": None}}]
    admission.api = lambda path: prs
    requests = []
    fail = True

    def github(method, path, payload):
        requests.append((method, path, payload))
        if fail:
            raise RuntimeError("transient PR API failure")
        return {"html_url": "https://github.com/owner/repo/pull/1"}

    admission.github = github
    try:
        admission.apply_pr()
        raise AssertionError("expected the injected PR creation failure")
    except RuntimeError as error:
        assert "transient" in str(error)
    branch = requests[0][2]["head"]
    pending = git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0]
    # A lookalike orphan is not a recovery receipt, even at the right ref/tree.
    message = git("show", "-s", "--format=%B", pending)
    prepared = os.environ["SIMIT_RELEASE_PR_TREE"]
    raw = f"tree {prepared}\nparent {sha}\nauthor Fixture <fixture@example.invalid> 1700000000 +0000\ncommitter Fixture <fixture@example.invalid> 1700000000 +0000\n\n{message}\n"
    unsigned = git("hash-object", "-t", "commit", "--stdin", "-w", input=raw)
    bad_heads = [unsigned]
    for candidate_tree, parent, candidate_message in [
        (tree, sha, message), (prepared, pending, message), (prepared, sha, message + "\nmanual amendment"),
    ]:
        bad_heads.append(git("commit-tree", "-S", candidate_tree, "-p", parent, input=candidate_message + "\n"))
    for candidate in bad_heads:
        git("push", "--force", "origin", f"{candidate}:refs/heads/{branch}")
        before = len(requests)
        try:
            admission.apply_pr()
            raise AssertionError("resumed an unsigned or source/body-mismatched orphan")
        except (RuntimeError, subprocess.CalledProcessError):
            pass
        assert len(requests) == before
    git("--git-dir", str(remote), "update-ref", f"refs/heads/{branch}", pending)
    fail = False
    remote_sha = "f" * 40
    before = len(requests)
    admission.apply_pr()
    assert len(requests) == before, "a stale retry must not create a PR"
    remote_sha = sha
    admission.apply_pr()
    assert requests[-1][0] == "POST"
    generated = git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0]
    assert generated == pending, "retry must reuse the already-pushed signed commit"
    git("verify-commit", generated)
    assert "0.2.0" in git("show", f"{generated}:Cargo.toml")
    assert "Prepared by release-plz" in git("show", f"{generated}:CHANGELOG.md")
    assert git("rev-parse", "HEAD") == sha
    prs.insert(0, {"number": 1, "body": requests[-1][2]["body"], "head": {"ref": branch, "sha": generated, "repo": {"full_name": "owner/repo"}}})
    delivered = len(requests)
    admission.apply_pr()
    assert len(requests) == delivered, "a successful delivery with a lost response must be a no-op"
    (runtime / "release-plz-pr/summary.txt").write_text("updated release summary\n")
    fail = True
    previous_body = prs[0]["body"]
    try:
        admission.apply_pr()
        raise AssertionError("expected the injected PR update failure")
    except RuntimeError as error:
        assert "transient" in str(error)
    pending = git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0]
    assert pending != generated
    prs[0]["head"]["sha"] = pending  # GitHub observes the push even though PATCH failed.
    prs[0]["body"] = previous_body + "\n\nOwner's manual notes."
    before = len(requests)
    try:
        admission.apply_pr()
        raise AssertionError("retry overwrote manual edits after a failed PATCH")
    except RuntimeError:
        pass
    assert len(requests) == before
    prs[0]["body"] = previous_body
    try:
        admission.apply_pr()
        raise AssertionError("expected a second injected PR update failure")
    except RuntimeError as error:
        assert "transient" in str(error)
    assert git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0] == pending
    next_source = git("commit-tree", tree, "-p", sha, input="next default-branch source\n")
    remote_sha = next_source
    before = len(requests)
    admission.apply_pr()
    assert len(requests) == before, "a superseded PATCH retry must not mutate the PR"
    git("update-ref", "refs/heads/trunk", next_source)
    os.environ["GITHUB_SHA"] = next_source
    (runtime / "release-plz-pr/summary.txt").write_text("current-source release summary\n")
    fail = False
    admission.apply_pr()
    assert requests[-1][0] == "PATCH", "automatic refresh should reuse the existing PR"
    generated = git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0]
    assert generated != pending
    assert git("show", "-s", "--format=%P", generated) == next_source
    assert "current-source release summary" in requests[-1][2]["body"]
    delivered = len(requests)
    prs[0]["head"]["sha"] = generated
    prs[0]["body"] = requests[-1][2]["body"] + "\n\nOwner's manual notes."
    try:
        admission.apply_pr()
        raise AssertionError("overwrote manual PR body edits")
    except RuntimeError:
        pass
    assert len(requests) == delivered
    prs[0]["body"] = requests[-1][2]["body"]
    prs[0]["head"]["sha"] = sha
    try:
        admission.apply_pr()
        raise AssertionError("overwrote a manually changed release PR")
    except RuntimeError:
        pass
    assert git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0] == generated
    prs[0]["head"]["sha"] = generated
    prs[0]["body"] = requests[-1][2]["body"]
    (runtime / "release-plz-pr/summary.txt").write_text("superseded prepared update\n")
    remote_sha = "f" * 40
    admission.apply_pr()
    assert len(requests) == delivered, "stale source must not replace a newer release PR"
    admission.subprocess.check_output = original_check_output
    subprocess.run(["gpgconf", "--kill", "gpg-agent"], check=True)

    git("reset", "--hard", sha)
    os.environ["GITHUB_SHA"] = sha
    # A valid no-op hunk can come from an untrusted worker even though git diff
    # would normally emit an empty patch. Do not sign an empty release commit.
    (runtime / "release-plz-pr/update.patch").write_text(
        'diff --git a/Cargo.toml b/Cargo.toml\n--- a/Cargo.toml\n+++ b/Cargo.toml\n'
        '@@ -1,3 +1,3 @@\n [package]\n-name="fixture"\n+name="fixture"\n version="0.1.0"\n'
    )
    (runtime / "env").write_text("")
    admission.admit_pr()
    assert (runtime / "env").read_text() == "SIMIT_RELEASE_PR_TREE=\n"

    # Reject workflow changes, links, and deleted manifests before importing keys.
    for kind in ("workflow", "link", "deletion", "workflow-rename", "release-rename"):
        git("reset", "--hard", sha)
        if kind == "workflow":
            path = root / ".github/workflows/evil.yaml"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("jobs: {}\n")
            git("add", "--intent-to-add", str(path))
        elif kind == "link":
            (root / "Cargo.lock").symlink_to("/proc/self/environ")
            git("add", "--intent-to-add", "Cargo.lock")
        elif kind == "deletion":
            (root / "Cargo.toml").unlink()
        else:
            (root / "x").mkdir(exist_ok=True)
            source = ".github/workflows/ci.yml" if kind == "workflow-rename" else "old/Cargo.lock"
            git("mv", source, "x/Cargo.lock")
        bad = subprocess.check_output(["git", "diff", "--binary", "HEAD"])
        git("reset", "--hard", sha)
        (runtime / "release-plz-pr/update.patch").write_bytes(bad)
        try:
            admission.admit_pr()
            raise AssertionError(f"admitted {kind} patch")
        except RuntimeError:
            pass

print("release-plz PR data admission, signed delivery and manual-edit protection passed")
