"""Real workspace packaging, independent reconstruction, and hostile Cargo config."""

import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

engine = str(Path(sys.argv[1]).resolve())


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    source = root / "source"
    source.mkdir()
    run("git", "init", "-b", "trunk", str(source))
    (source / "Cargo.toml").write_text('[workspace]\nmembers=["base", "dependent"]\nresolver="2"\n')
    marker = root / "executed-build-code"
    for name in ("base", "dependent"):
        package = source / name
        (package / "src").mkdir(parents=True)
        dependencies = "" if name == "base" else '[dependencies]\nbase={package="simit-release-plz-fixture-base",version="=0.1.0",path="../base"}\n'
        (package / "Cargo.toml").write_text(f'[package]\nname="simit-release-plz-fixture-{name}"\nversion="0.1.0"\nedition="2021"\nlicense="MIT"\ndescription="Disposable release admission fixture"\n{dependencies}')
        (package / "src/lib.rs").write_text("pub fn answer() -> u32 { 42 }\n")
        (package / "build.rs").write_text(f'fn main() {{ assert!(std::env::var_os("CRATES_IO_API_TOKEN").is_none()); std::fs::write({json.dumps(str(marker))}, "build code ran").unwrap(); }}\n')
    (source / ".cargo").mkdir()
    wrapper = source / "steal.sh"
    wrapper.write_text(f'#!/bin/sh\nprintf "%s" "$CRATES_IO_API_TOKEN" > "{marker}"\nexit 1\n')
    wrapper.chmod(0o755)
    (source / ".cargo/config.toml").write_text(f'[build]\nrustc-wrapper={json.dumps(str(wrapper))}\n')
    home = root / "home"
    home.mkdir()
    clean = {
        "PATH": os.environ["PATH"], "HOME": str(home), "CARGO_HOME": str(home / "cargo"),
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
    }
    run("cargo", "generate-lockfile", "--offline", "--manifest-path", str(source / "Cargo.toml"), cwd="/", env=clean)
    run("git", "-C", str(source), "add", ".")
    tree = run("git", "-C", str(source), "write-tree")
    commit = f"tree {tree}\nauthor Fixture <fixture@example.invalid> 1700000000 +0000\ncommitter Fixture <fixture@example.invalid> 1700000000 +0000\n\nrelease\n"
    sha = run("git", "-C", str(source), "hash-object", "-t", "commit", "--stdin", "-w", input=commit)
    run("git", "-C", str(source), "update-ref", "refs/heads/trunk", sha)
    target = root / "target"
    run("cargo", "package", "--workspace", "--locked", "--offline", "--target-dir", str(target), "--manifest-path", str(source / "Cargo.toml"), cwd="/", env=clean)
    assert marker.exists(), "fixture never exercised compilation"
    marker.unlink()
    event = root / "event.json"
    event.write_text(json.dumps({"repository": {"full_name": "owner/repo", "default_branch": "trunk"}}))
    env = {
        **os.environ, "GITHUB_EVENT_PATH": str(event), "GITHUB_EVENT_NAME": "push",
        "GITHUB_REF": "refs/heads/trunk", "GITHUB_REPOSITORY": "owner/repo",
        "GITHUB_SHA": sha, "SIMIT_EXPECTED_RELEASE_SHA": sha,
        "SIMIT_RELEASE_PLZ_SOURCE": str(source), "CRATES_IO_API_TOKEN": "fixture-registry-secret",
        "GIT_TOKEN": "fixture-write-secret", "RELEASE_KEY": "fixture-signing-secret",
    }
    command = [engine, "release", "publisher", "--publisher-stage", "prepare", "--repo", "owner/repo", "--release-plz-archives", str(target / "package")]
    result = subprocess.run(command, env=env, capture_output=True)
    assert result.returncode == 0, result.stderr.decode()
    assert not marker.exists(), "independent admission executed project code with secrets"
    archive = target / "package/simit-release-plz-fixture-base-0.1.0.crate"
    original = archive.read_bytes()
    output = io.BytesIO()
    with tarfile.open(fileobj=io.BytesIO(original), mode="r:gz") as incoming, tarfile.open(fileobj=output, mode="w:gz") as outgoing:
        for member in incoming:
            content = incoming.extractfile(member).read()
            if member.name.endswith("/src/lib.rs"):
                content += b"pub fn injected() {}\n"
                member.size = len(content)
            outgoing.addfile(member, io.BytesIO(content))
    archive.write_bytes(output.getvalue())
    result = subprocess.run(command, env=env, capture_output=True)
    assert result.returncode != 0, "admitted a changed package archive"
    assert not marker.exists()

print("release-plz workspace archives verified without project code or credential inheritance")
