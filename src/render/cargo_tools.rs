//! Pinned native Cargo tools for runners without the Nix dev shell.

use std::fmt::Write as _;

const AUDIT_VERSION: &str = "0.23.0";
const DENY_VERSION: &str = "0.18.3";
const NEXTEST_VERSION: &str = "0.9.100";
const NEXTEST_DARWIN_SHA256: &str =
    "0d8c1fc024a6a72ebe8a76f3db9220f8c01c7247dc3e68747838393bb7adc957";

#[derive(Clone, Copy)]
pub(super) enum Tool {
    Audit,
    Deny,
    Nextest,
}

impl Tool {
    fn installer(self, dir: &str) -> String {
        let (name, base_url, suffix, assets) = match self {
            Self::Audit => (
                "cargo-audit",
                format!(
                    "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v{AUDIT_VERSION}"
                ),
                ".tgz",
                [
                    (
                        "x86_64-unknown-linux-musl",
                        "4f01c74ed84b2dffd0b1d37012a62cdf2a92c96e25bdf8eec7fcc0d4d3eb7811",
                    ),
                    (
                        "aarch64-unknown-linux-gnu",
                        "00eff3158429b9629abe8748e971f29f99ee53d4cff54a25072cbb15167bdda8",
                    ),
                    (
                        "x86_64-apple-darwin",
                        "845ec02452dd233c292be887700ac99a38a697299423081ee0d8ea1a0c961464",
                    ),
                    (
                        "aarch64-apple-darwin",
                        "84101204e5b8013c9a8d7251766aebdef3e7fa1a3ef040cde44d65b48b8c4056",
                    ),
                ],
            ),
            Self::Deny => (
                "cargo-deny",
                format!(
                    "https://github.com/EmbarkStudios/cargo-deny/releases/download/{DENY_VERSION}"
                ),
                ".tar.gz",
                [
                    (
                        "x86_64-unknown-linux-musl",
                        "5037f3c167a8da8cea04c34a89e74cef95c646f2a537750b2db58e54f6a788e7",
                    ),
                    (
                        "aarch64-unknown-linux-musl",
                        "8d82b10962ccd64e82744ccedc77209d728393b5be2c66300ba9009705901041",
                    ),
                    (
                        "x86_64-apple-darwin",
                        "bb819f7e2b7ac74e38fb61ccdf2cf741c653c5f506d29dfd1941604abb4e4ac0",
                    ),
                    (
                        "aarch64-apple-darwin",
                        "9af127b035c25e2a8bce212d8e40d6e22f25361868fcd740db6096320762ed56",
                    ),
                ],
            ),
            Self::Nextest => (
                "cargo-nextest",
                format!(
                    "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-{NEXTEST_VERSION}"
                ),
                ".tar.gz",
                [
                    (
                        "x86_64-unknown-linux-musl",
                        "fbc4f0989a1b58bc23c335f8e1dc656f60efb854246aaa869a88543494e5b0ce",
                    ),
                    (
                        "aarch64-unknown-linux-gnu",
                        "53c2e65b61a3736304b3784f1300aaef61fc1538e0b36b7bc7461d4a1c690556",
                    ),
                    ("universal-apple-darwin", NEXTEST_DARWIN_SHA256),
                    ("universal-apple-darwin", NEXTEST_DARWIN_SHA256),
                ],
            ),
        };
        let mut script = String::from(
            "set -eu\nkernel=\"$(uname -s)\"\narch=\"$(uname -m)\"\ncase \"$kernel/$arch\" in\n",
        );
        for (platform, (target, sha256)) in [
            "Linux/x86_64|Linux/amd64",
            "Linux/aarch64|Linux/arm64",
            "Darwin/x86_64|Darwin/amd64",
            "Darwin/aarch64|Darwin/arm64",
        ]
        .into_iter()
        .zip(assets)
        {
            let stem = match self {
                Self::Audit => format!("{name}-{target}-v{AUDIT_VERSION}"),
                Self::Deny => format!("{name}-{DENY_VERSION}-{target}"),
                Self::Nextest => format!("{name}-{NEXTEST_VERSION}-{target}"),
            };
            if target == "aarch64-unknown-linux-gnu" {
                writeln!(
                    script,
                    r#"  {platform})
    test -x /lib/ld-linux-aarch64.so.1 || {{ echo "{name} on Linux/aarch64 requires glibc (/lib/ld-linux-aarch64.so.1); musl-only runners are unsupported. Use a glibc-based runner." >&2; exit 1; }}
    stem={stem}; sha256={sha256} ;;"#
                )
                .expect("write");
            } else {
                writeln!(script, "  {platform}) stem={stem}; sha256={sha256} ;;").expect("write");
            }
        }
        let member = match self {
            Self::Audit | Self::Deny => format!("$stem/{name}"),
            Self::Nextest => name.to_owned(),
        };
        writeln!(
            script,
            r#"  *) echo "Unsupported {name} platform: $kernel/$arch" >&2; exit 1 ;;
esac
bin="$(mktemp -d "{dir}")"
trap 'rm -rf -- "$bin"' EXIT
archive="$bin/$stem{suffix}"
curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --retry 3 \
  --connect-timeout 10 --max-time 120 --max-filesize 200000000 -o "$archive" "{base_url}/$stem{suffix}"
case "$kernel" in
  Linux) printf '%s  %s\n' "$sha256" "$archive" | sha256sum -c - ;;
  Darwin) printf '%s  %s\n' "$sha256" "$archive" | shasum -a 256 -c - ;;
esac
tar -xOzf "$archive" -- "{member}" > "$bin/{name}"
rm -- "$archive"
chmod 0755 "$bin/{name}"
export PATH="$bin:$PATH"
test "$(command -v {name})" = "$bin/{name}" || {{ echo "PATH did not resolve {name} to the verified $bin/{name}" >&2; exit 1; }}
# No fallback to a preinstalled executable if this mount is noexec.
PATH="$bin" {name} --version"#
        )
        .expect("write");
        script
    }

    pub(super) fn push_actions(self, workflow: &mut String, step_name: &str) {
        writeln!(workflow, "      - name: {step_name}\n        run: |").expect("write");
        let script = self.installer("${RUNNER_TEMP:-${TMPDIR:-/tmp}}/simit-cargo-tools.XXXXXXXX");
        for line in script.lines() {
            writeln!(workflow, "          {line}").expect("write");
        }
        workflow.push_str(
            "          printf '%s\\n' \"$bin\" >> \"$GITHUB_PATH\"\n          trap - EXIT\n\n",
        );
    }

    pub(super) fn crow(self, command: &str) -> String {
        // Each container gets its own CARGO_HOME and PATH. Install, probe and
        // use the tool in the same step, on the executable workspace mount.
        let mut script = self.installer("$PWD/.simit-cargo-tools.XXXXXXXX");
        script.push('\n');
        script.push_str(command);
        script.replace('$', "$$")
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    use flate2::{Compression, write::GzEncoder};
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn glibc_only_arm64_installers_fail_before_download_without_loader() {
        let root = tempfile::tempdir().unwrap();
        for (tool, name) in [
            (Tool::Audit, "cargo-audit"),
            (Tool::Nextest, "cargo-nextest"),
        ] {
            let script = tool.installer("$PWD/.simit-cargo-tools.XXXXXXXX");
            assert!(script.contains("test -x /lib/ld-linux-aarch64.so.1"));
            let script = script.replace("/lib/ld-linux-aarch64.so.1", "./missing-glibc-loader");
            let out = Command::new("sh")
                .args([
                    "-c",
                    &format!(
                        "uname() {{ case \"$1\" in -s) echo Linux;; -m) echo aarch64;; esac; }}\n\
                         curl() {{ echo 'unexpected download' >&2; return 1; }}\n{script}"
                    ),
                ])
                .current_dir(root.path())
                .env("HOME", "/homeless-shelter")
                .output()
                .unwrap();
            assert!(!out.status.success());
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                stderr.contains(&format!("{name} on Linux/aarch64 requires glibc")),
                "{stderr}"
            );
            assert!(stderr.contains("Use a glibc-based runner"), "{stderr}");
            assert!(!stderr.contains("unexpected download"), "{stderr}");
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn installer_verifies_before_extraction_and_publishes_only_on_success() {
        let stubs = r#"
uname() { case "$1" in -s) printf '%s\n' "$KERNEL";; -m) printf '%s\n' "$ARCH";; esac; }
curl() {
    echo 'downloaded fixture' >&2
    while [ "$1" != -o ]; do shift; done
    test "$3" = "$URL" || return 40
    cp "$FIXTURE" "$2"
}
tar() { echo 'extracting fixture' >&2; command tar "$@"; }
"#;
        for (tool, name, version, stem, suffix, member, pin, url) in [
            (
                Tool::Audit,
                "cargo-audit",
                "0.23.0",
                "cargo-audit-x86_64-unknown-linux-musl-v0.23.0",
                ".tgz",
                "cargo-audit-x86_64-unknown-linux-musl-v0.23.0/cargo-audit",
                "4f01c74ed84b2dffd0b1d37012a62cdf2a92c96e25bdf8eec7fcc0d4d3eb7811",
                "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v0.23.0",
            ),
            (
                Tool::Deny,
                "cargo-deny",
                "0.18.3",
                "cargo-deny-0.18.3-x86_64-unknown-linux-musl",
                ".tar.gz",
                "cargo-deny-0.18.3-x86_64-unknown-linux-musl/cargo-deny",
                "5037f3c167a8da8cea04c34a89e74cef95c646f2a537750b2db58e54f6a788e7",
                "https://github.com/EmbarkStudios/cargo-deny/releases/download/0.18.3",
            ),
            (
                Tool::Nextest,
                "cargo-nextest",
                "0.9.100",
                "cargo-nextest-0.9.100-x86_64-unknown-linux-musl",
                ".tar.gz",
                "cargo-nextest",
                "fbc4f0989a1b58bc23c335f8e1dc656f60efb854246aaa869a88543494e5b0ce",
                "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.100",
            ),
        ] {
            let mut setup = String::new();
            tool.push_actions(&mut setup, "Install fixture");
            let (_, script) = setup.split_once("run: |\n").unwrap();
            assert!(script.contains(&format!("stem={stem}; sha256={pin}")));
            let root = tempfile::tempdir().unwrap();
            let fixture = root.path().join("fixture.tar.gz");
            let path_file = root.path().join("github-path");
            let runner = root.path().join("runner temp");
            fs::create_dir(&runner).unwrap();
            let preinstalled = root.path().join(name);
            fs::write(
                &preinstalled,
                "#!/bin/sh\necho 'untrusted preinstalled tool'\n",
            )
            .unwrap();
            fs::set_permissions(&preinstalled, fs::Permissions::from_mode(0o755)).unwrap();
            let inherited = std::env::var_os("PATH").unwrap();
            let path = std::env::join_paths(
                std::iter::once(root.path().to_path_buf()).chain(std::env::split_paths(&inherited)),
            )
            .unwrap();
            for (kernel, arch, correct_hash, version_works) in [
                ("FreeBSD", "x86_64", false, true),
                ("Linux", "riscv64", false, true),
                ("Linux", "x86_64", false, true),
                ("Linux", "x86_64", true, false),
                ("Linux", "x86_64", true, true),
            ] {
                let contents = if version_works {
                    format!("#!/bin/sh\n[ \"$*\" = --version ] && echo '{name} {version}'\n")
                } else {
                    "#!/nonexistent/loader\n".to_owned()
                };
                let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
                let mut header = tar::Header::new_gnu();
                header.set_size(contents.len().try_into().unwrap());
                header.set_mode(0o644);
                tar.append_data(&mut header, member, contents.as_bytes())
                    .unwrap();
                let bytes = tar.into_inner().unwrap().finish().unwrap();
                fs::write(&fixture, &bytes).unwrap();
                let script = if correct_hash {
                    script.replace(pin, &format!("{:x}", Sha256::digest(&bytes)))
                } else {
                    script.to_owned()
                };
                let out = Command::new("sh")
                    .args(["-c", &format!("{stubs}\n{script}")])
                    .env("HOME", root.path())
                    .env("PATH", &path)
                    .env("RUNNER_TEMP", &runner)
                    .env("GITHUB_PATH", &path_file)
                    .env("KERNEL", kernel)
                    .env("ARCH", arch)
                    .env("URL", format!("{url}/{stem}{suffix}"))
                    .env("FIXTURE", &fixture)
                    .output()
                    .unwrap();
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert_eq!(
                    stderr.contains("downloaded fixture"),
                    kernel == "Linux" && arch == "x86_64",
                    "{stderr}"
                );
                assert_eq!(
                    stderr.contains("extracting fixture"),
                    correct_hash,
                    "{stderr}"
                );
                let success = correct_hash && version_works;
                assert_eq!(
                    out.status.success(),
                    success,
                    "{name} {kernel}/{arch}: {stderr}"
                );
                assert_eq!(path_file.exists(), success);
                assert_eq!(fs::read_dir(&runner).unwrap().count(), usize::from(success));
                assert!(
                    !String::from_utf8_lossy(&out.stdout).contains("untrusted preinstalled tool")
                );
                if success {
                    assert!(
                        String::from_utf8_lossy(&out.stdout).contains(&format!("{name} {version}"))
                    );
                    let bin = fs::read_to_string(&path_file).unwrap();
                    let entries: Vec<_> = fs::read_dir(bin.trim()).unwrap().collect();
                    assert_eq!(entries.len(), 1, "download must be removed");
                    assert_eq!(entries[0].as_ref().unwrap().file_name(), name);
                }
            }
        }
    }
}
