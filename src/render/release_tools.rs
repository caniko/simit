//! Native Linux tools shared by the release publishers.

use std::fmt::Write as _;

const MINISIGN_VERSION: &str = "0.12";
const MINISIGN_SHA256: &str = "9a599b48ba6eb7b1e80f12f36b94ceca7c00b7a5173c95c3efc88d9822957e73";
const COSIGN_VERSION: &str = "v3.1.4";
const COSIGN_AMD64_SHA256: &str =
    "424e2c97d0b45fd7a6b608a87f090f35633f4c23ece5f092d48bba11aaf4bbfa";
const COSIGN_ARM64_SHA256: &str =
    "ade83b7aac62e131cacfe96d958948b2150cc9efec46a570a2649b8e27dedad2";

pub(super) fn push_setup(w: &mut String, commands: &[&str], minisign: bool, cosign: bool) {
    let mut required = commands.to_vec();
    if minisign || cosign {
        required.extend(["curl", "sha256sum", "uname"]);
    }
    if minisign {
        required.push("tar");
    }
    required.sort_unstable();
    required.dedup();
    let required = required.join(" ");
    w.push_str(
        "      - name: Set up native release tools\n        run: |\n          set -euo pipefail\n",
    );
    writeln!(
        w,
        r#"          for tool in {required}; do
            command -v "$tool" >/dev/null || {{ echo "::error::Required release tool not found: $tool" >&2; exit 1; }}
          done"#
    )
    .expect("write");
    if !minisign && !cosign {
        return;
    }
    w.push_str(
        r#"          kernel="$(uname -s)"
          arch="$(uname -m)"
          case "$kernel/$arch" in
            Linux/x86_64|Linux/amd64) arch=x86_64 ;;
            Linux/aarch64|Linux/arm64) arch=aarch64 ;;
            *) echo "::error::Unsupported release tool platform: $kernel/$arch" >&2; exit 1 ;;
          esac
          bin="$(mktemp -d "$RUNNER_TEMP/simit-release-tools.XXXXXXXX")"
          trap 'rm -f -- "$bin/minisign.tar.gz" "$bin/cosign.download"' EXIT
          download() {
            curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --retry 3 \
              --connect-timeout 10 --max-time 120 --max-filesize 200000000 -o "$2" "$1"
          }
"#,
    );
    if minisign {
        writeln!(
            w,
            r#"          archive="$bin/minisign.tar.gz"
          download 'https://github.com/jedisct1/minisign/releases/download/{MINISIGN_VERSION}/minisign-{MINISIGN_VERSION}-linux.tar.gz' "$archive"
          printf '%s  %s\n' '{MINISIGN_SHA256}' "$archive" | sha256sum --check --strict
          tar -xzf "$archive" --strip-components=2 -C "$bin" -- "minisign-linux/$arch/minisign"
          "$bin/minisign" -v"#
        )
        .expect("write");
    }
    if cosign {
        writeln!(
            w,
            r#"          case "$arch" in
            x86_64) target=amd64; sha256={COSIGN_AMD64_SHA256} ;;
            aarch64) target=arm64; sha256={COSIGN_ARM64_SHA256} ;;
          esac
          binary="$bin/cosign.download"
          download "https://github.com/sigstore/cosign/releases/download/{COSIGN_VERSION}/cosign-linux-$target" "$binary"
          printf '%s  %s\n' "$sha256" "$binary" | sha256sum --check --strict
          chmod 0755 "$binary"
          mv -- "$binary" "$bin/cosign"
          "$bin/cosign" version"#
        )
        .expect("write");
    }
    w.push_str("          printf '%s\\n' \"$bin\" >> \"$GITHUB_PATH\"\n");
}

/// Detect literal Nix invocations in configured shell snippets, not comments or
/// arguments such as `echo 'nix build'`.
/// ponytail: this cannot inspect external scripts or computed command names;
/// use a literal Nix entry point in workflow configuration for those.
pub(super) fn command_uses_nix(run: &str) -> bool {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("compatible Bash grammar");
    let tree = parser.parse(run, None).expect("parse shell snippet");
    let mut nodes = vec![tree.root_node()];
    while let Some(node) = nodes.pop() {
        if node.kind() == "command" {
            let mut cursor = node.walk();
            let mut words = node
                .child_by_field_name("name")
                .into_iter()
                .chain(node.children_by_field_name("argument", &mut cursor))
                .map(|word| {
                    let mut literal = shlex::split(run.get(word.byte_range())?)?;
                    (literal.len() == 1).then(|| literal.remove(0))
                })
                .peekable();
            'launch: while let Some(Some(word)) = words.next() {
                match word.rsplit('/').next().unwrap_or(&word) {
                    "nix" | "nix-shell" | "nix-build" | "nix-env" | "nix-store"
                    | "nix-instantiate" | "nix-channel" | "nix-copy-closure"
                    | "nix-prefetch-url" => return true,
                    "env" | "command" | "exec" | "sudo" => {
                        while let Some(Some(option)) = words.next_if(|word| {
                            word.as_deref()
                                .is_some_and(|word| word.starts_with('-') || word.contains('='))
                        }) {
                            match option.as_str() {
                                "-u" | "--unset" | "-g" | "--group" | "--user" | "-C"
                                | "--chdir" | "-a" => {
                                    words.next();
                                }
                                "-v" | "-V" if word == "command" => break 'launch,
                                _ => {}
                            }
                        }
                    }
                    "bash" | "sh" => {
                        while let Some(Some(flag)) = words.next() {
                            if !flag.starts_with('-') || flag == "--" {
                                break;
                            }
                            if !flag.starts_with("--") && flag.contains('c') {
                                if let Some(Some(script)) = words.next()
                                    && command_uses_nix(&script)
                                {
                                    return true;
                                }
                                break;
                            }
                        }
                        break;
                    }
                    _ => break,
                }
            }
        }
        nodes.extend(node.named_children(&mut node.walk()));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_commands_detect_executable_nix_not_literal_data() {
        for snippet in [
            "nix run .#smoke",
            "if true; then nix-shell --run smoke; fi",
            "out=$(nix build --print-out-paths)",
            "ci=1 /run/current-system/sw/bin/nix eval .",
            "env CI=1 bash -lc 'nix develop -c cargo check'",
            "true && command -- nix-store --gc",
        ] {
            assert!(command_uses_nix(snippet), "{snippet}");
        }
        for snippet in [
            "# nix develop\nscripts/build",
            "echo 'nix build' && jq '.nix'",
            "command -v nix",
            "bash -c 'echo nix'",
        ] {
            assert!(!command_uses_nix(snippet), "{snippet}");
        }
    }

    #[test]
    fn native_release_tools_verify_downloads_before_using_them() {
        let mut setup = String::new();
        push_setup(&mut setup, &["git", "jq"], true, true);
        for (verify, use_it, probe) in [
            (
                "\"$archive\" | sha256sum --check --strict",
                "tar -xzf",
                "\"$bin/minisign\" -v",
            ),
            (
                "\"$binary\" | sha256sum --check --strict",
                "chmod 0755",
                "\"$bin/cosign\" version",
            ),
        ] {
            assert!(setup.find(verify).unwrap() < setup.find(use_it).unwrap());
            assert!(setup.find(use_it).unwrap() < setup.find(probe).unwrap());
            assert!(setup.find(probe).unwrap() < setup.find(">> \"$GITHUB_PATH\"").unwrap());
        }
        assert!(setup.contains("-- \"minisign-linux/$arch/minisign\""));
        assert!(setup.contains("Linux/x86_64|Linux/amd64"));
        assert!(setup.contains("Linux/aarch64|Linux/arm64"));
        assert!(setup.contains(&format!("target=amd64; sha256={COSIGN_AMD64_SHA256}")));
        assert!(setup.contains(&format!("target=arm64; sha256={COSIGN_ARM64_SHA256}")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_release_tools_refuse_bad_hashes_and_unknown_architectures() {
        for tool in ["bash", "sha256sum"] {
            if std::process::Command::new(tool)
                .arg("--version")
                .output()
                .is_err()
            {
                return;
            }
        }
        let stubs = r#"
uname() { case "$1" in -s) echo Linux;; -m) echo "$TEST_ARCH";; esac; }
curl() {
  echo 'downloading fixture' >&2
  while [ "$1" != -o ]; do shift; done
  printf 'untrusted binary\n' > "$2"
}
tar() { echo 'unexpected extraction' >&2; }
chmod() { echo 'unexpected chmod' >&2; }
"#;
        for (arch, minisign, cosign, downloaded) in [
            ("riscv64", true, true, false),
            ("amd64", true, false, true),
            ("arm64", false, true, true),
        ] {
            let runner = tempfile::tempdir().unwrap();
            let path_file = runner.path().join("github-path");
            let mut setup = String::new();
            push_setup(&mut setup, &[], minisign, cosign);
            let (_, script) = setup.split_once("run: |\n").unwrap();
            let output = std::process::Command::new("bash")
                .args(["-c", &format!("{stubs}{script}")])
                .env("HOME", runner.path())
                .env("RUNNER_TEMP", runner.path())
                .env("GITHUB_PATH", &path_file)
                .env("TEST_ARCH", arch)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{arch}");
            assert_eq!(
                stderr.contains("downloading fixture"),
                downloaded,
                "{stderr}"
            );
            assert_eq!(
                stderr.contains("Unsupported release tool platform"),
                !downloaded,
                "{stderr}"
            );
            assert!(!stderr.contains("unexpected "), "{stderr}");
            assert!(!path_file.exists());
            for entry in std::fs::read_dir(runner.path()).unwrap() {
                assert_eq!(std::fs::read_dir(entry.unwrap().path()).unwrap().count(), 0);
            }
        }
    }
}
