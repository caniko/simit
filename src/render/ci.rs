use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::cargo::Package;
use crate::cli::{Platform, Runtime};
use crate::project::GeneratedFile;

#[derive(Debug, Clone, Default)]
pub struct CiOptions {
    pub with_nextest: bool,
    pub with_msrv: bool,
    pub with_audit: bool,
    pub with_deny: bool,
    pub with_docs: bool,
    pub with_artifacts: bool,
    pub homebrew: Option<HomebrewOptions>,
}

#[derive(Debug, Clone)]
pub struct HomebrewOptions {
    /// Formula name (e.g. "modde"). UpperCamelCased for the Ruby class.
    pub name: String,
    /// Binaries to install. Defaults to [name] if empty.
    pub binaries: Vec<String>,
    /// Tap repo URL (e.g. "https://codeberg.org/caniko/homebrew-modde.git").
    pub tap_url: String,
    /// Project description for the formula (<= 80 chars).
    pub description: String,
    /// Homepage URL.
    pub homepage: String,
    /// SPDX license identifier.
    pub license: String,
    /// Release-artefact filename pattern. Supports {version}, {arch}, {os} placeholders.
    pub archive_pattern: String,
    /// Codeberg user/repo for download URLs.
    pub download_repo: String,
    /// Per-platform enable/disable. Default: all four.
    pub platforms: HomebrewPlatformSet,
}

#[derive(Debug, Clone)]
pub struct HomebrewPlatformSet {
    pub darwin_arm: bool,
    pub darwin_intel: bool,
    pub linux_arm: bool,
    pub linux_intel: bool,
}

impl Default for HomebrewPlatformSet {
    fn default() -> Self {
        Self {
            darwin_arm: true,
            darwin_intel: true,
            linux_arm: true,
            linux_intel: true,
        }
    }
}

pub fn files(
    platform: Platform,
    runtime: Runtime,
    package: &Package,
    self_check: bool,
    runner_override: Option<&str>,
    options: CiOptions,
) -> Result<Vec<GeneratedFile>> {
    if options.with_msrv && package.rust_version.is_none() {
        bail!("--with-msrv requires package.rust-version in Cargo.toml");
    }

    let dir = PathBuf::from(platform.workflow_dir());
    let mut files = vec![
        GeneratedFile {
            relative_path: dir.join("ci.yaml"),
            content: ci_workflow(
                platform,
                runtime,
                package,
                self_check,
                runner_override,
                options.clone(),
            ),
        },
        GeneratedFile {
            relative_path: dir.join("publish-crate.yaml"),
            content: publish_workflow(platform, runtime, package, runner_override, options.clone()),
        },
    ];

    if options.with_artifacts {
        files.push(GeneratedFile {
            relative_path: dir.join("release-artifacts.yaml"),
            content: artifacts_workflow(platform, runtime, runner_override, &options),
        });
    }
    if options.with_deny {
        files.push(GeneratedFile {
            relative_path: PathBuf::from("deny.toml"),
            content: deny_toml(),
        });
    }

    Ok(files)
}

fn ci_workflow(
    platform: Platform,
    runtime: Runtime,
    package: &Package,
    self_check: bool,
    runner_override: Option<&str>,
    options: CiOptions,
) -> String {
    let mut workflow = String::new();
    workflow.push_str("name: CI\n\n");
    workflow.push_str("on:\n");
    workflow.push_str("  push:\n");
    workflow.push_str("    branches: [trunk]\n");
    workflow.push_str("  pull_request:\n\n");
    workflow.push_str("jobs:\n");
    workflow.push_str("  test:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(platform, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime);
    workflow.push_str("    steps:\n");
    push_checkout_step(&mut workflow, platform, runtime);

    match runtime {
        Runtime::Nix => {
            workflow.push_str("      - name: Install Nix\n");
            workflow.push_str("        uses: https://github.com/cachix/install-nix-action@v31\n\n");
            workflow.push_str("      - name: Check flake\n");
            workflow.push_str("        run: nix flake check\n\n");
            workflow.push_str("      - name: Test\n");
            workflow.push_str("        run: nix develop -c cargo test\n\n");
            push_quality_tool_install_steps(&mut workflow, runtime, &options);
            push_optional_ci_steps(&mut workflow, runtime, package, &options);
            if self_check {
                push_self_check_steps(&mut workflow, platform, runtime, runner_override, &options);
            }
            workflow.push_str("      - name: Clippy\n");
            workflow.push_str(
                "        run: nix develop -c cargo clippy --all-targets -- --deny warnings\n\n",
            );
            workflow.push_str("      - name: Package crate\n");
            workflow.push_str("        run: nix develop -c cargo package --allow-dirty\n");
        }
        Runtime::Cargo => {
            push_rust_setup_step(&mut workflow, platform);
            push_test_steps(&mut workflow, package, &options);
            push_quality_tool_install_steps(&mut workflow, runtime, &options);
            push_optional_ci_steps(&mut workflow, runtime, package, &options);
            if self_check {
                push_self_check_steps(&mut workflow, platform, runtime, runner_override, &options);
            }
            push_clippy_steps(&mut workflow, package);
            workflow.push_str("      - name: Package crate\n");
            workflow.push_str("        run: cargo package --allow-dirty\n");
        }
    }

    workflow
}

fn publish_workflow(
    platform: Platform,
    runtime: Runtime,
    package: &Package,
    runner_override: Option<&str>,
    options: CiOptions,
) -> String {
    let mut workflow = String::new();
    workflow.push_str(
        "# Before creating and pushing a release tag, run `simit changelog release <version>` locally.\n",
    );
    workflow.push_str("name: Publish Crate\n\n");
    workflow.push_str("on:\n");
    workflow.push_str("  push:\n");
    workflow.push_str("    tags:\n");
    workflow.push_str("      - \"*.*.*\"\n\n");
    workflow.push_str("jobs:\n");
    workflow.push_str("  publish:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(platform, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime);
    workflow.push_str("    steps:\n");
    push_checkout_step(&mut workflow, platform, runtime);

    match runtime {
        Runtime::Nix => {
            workflow.push_str("      - name: Install Nix\n");
            workflow.push_str("        uses: https://github.com/cachix/install-nix-action@v31\n\n");
            workflow.push_str(&validate_tag_step(
                "nix develop -c cargo metadata --no-deps --format-version 1",
            ));
            workflow.push_str("      - name: Check flake\n");
            workflow.push_str("        run: nix flake check\n\n");
            workflow.push_str("      - name: Test\n");
            workflow.push_str("        run: nix develop -c cargo test\n\n");
            push_quality_tool_install_steps(&mut workflow, runtime, &options);
            push_optional_publish_steps(&mut workflow, runtime, &options);
            workflow.push_str("      - name: Clippy\n");
            workflow.push_str(
                "        run: nix develop -c cargo clippy --all-targets -- --deny warnings\n\n",
            );
            workflow.push_str("      - name: Dry-run publish\n");
            workflow.push_str("        run: nix develop -c cargo publish --dry-run\n\n");
            workflow.push_str(&publish_step("nix develop -c cargo publish"));
        }
        Runtime::Cargo => {
            push_rust_setup_step(&mut workflow, platform);
            workflow.push_str(&validate_tag_step(
                "cargo metadata --no-deps --format-version 1",
            ));
            push_test_steps(&mut workflow, package, &options);
            push_quality_tool_install_steps(&mut workflow, runtime, &options);
            push_optional_publish_steps(&mut workflow, runtime, &options);
            push_clippy_steps(&mut workflow, package);
            workflow.push_str("      - name: Dry-run publish\n");
            workflow.push_str("        run: cargo publish --dry-run\n\n");
            workflow.push_str(&publish_step("cargo publish"));
        }
    }

    workflow
}

fn artifacts_workflow(
    platform: Platform,
    runtime: Runtime,
    runner_override: Option<&str>,
    options: &CiOptions,
) -> String {
    let mut workflow = String::new();
    workflow.push_str("name: Release Artifacts\n\n");
    workflow.push_str("on:\n");
    workflow.push_str("  push:\n");
    workflow.push_str("    tags:\n");
    workflow.push_str("      - \"*.*.*\"\n\n");
    workflow.push_str("jobs:\n");
    workflow.push_str("  build:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(platform, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime);
    workflow.push_str("    steps:\n");
    push_checkout_step(&mut workflow, platform, runtime);
    match runtime {
        Runtime::Nix => {
            workflow.push_str("      - name: Install Nix\n");
            workflow.push_str("        uses: https://github.com/cachix/install-nix-action@v31\n\n");
            workflow.push_str("      - name: Build package\n");
            workflow.push_str("        run: nix build\n\n");
        }
        Runtime::Cargo => {
            push_rust_setup_step(&mut workflow, platform);
            workflow.push_str("      - name: Build release binary\n");
            workflow.push_str("        run: cargo build --release --locked\n\n");
        }
    }
    if let Some(homebrew) = &options.homebrew {
        push_homebrew_publish_step(&mut workflow, homebrew);
    }
    workflow
}

fn push_homebrew_publish_step(workflow: &mut String, opts: &HomebrewOptions) {
    let tap_repo = homebrew_tap_repo(&opts.tap_url);
    let archives = homebrew_archives(opts);
    let binaries = if opts.binaries.is_empty() {
        vec![opts.name.as_str()]
    } else {
        opts.binaries.iter().map(String::as_str).collect()
    };

    workflow.push_str("      - name: Publish Homebrew tap\n");
    workflow.push_str("        env:\n");
    workflow.push_str("          HOMEBREW_TAP_TOKEN: ${{ secrets.homebrew_tap_token }}\n");
    workflow.push_str("          HOMEBREW_TAP_REPO: ");
    workflow.push_str(&tap_repo);
    workflow.push('\n');
    workflow.push_str("          HOMEBREW_TAP_URL: ");
    workflow.push_str(&opts.tap_url);
    workflow.push('\n');
    workflow.push_str("        run: |\n");
    workflow.push_str("          set -euo pipefail\n\n");
    workflow.push_str("          if [ -z \"${HOMEBREW_TAP_TOKEN:-}\" ]; then\n");
    workflow.push_str(
        "            echo \"HOMEBREW_TAP_TOKEN not configured; skipping Homebrew tap update.\"\n",
    );
    workflow.push_str("            exit 0\n");
    workflow.push_str("          fi\n\n");
    workflow.push_str("          VERSION=\"$CODEBERG_REF_NAME\"\n");
    workflow.push_str("          for artifact in \\\n");
    for (index, archive) in archives.iter().enumerate() {
        workflow.push_str("            \"release/");
        workflow.push_str(&archive.file_name);
        workflow.push('"');
        if index + 1 == archives.len() {
            workflow.push_str("; do\n");
        } else {
            workflow.push_str(" \\\n");
        }
    }
    workflow.push_str("            test -s \"$artifact\"\n");
    workflow.push_str("          done\n\n");
    workflow.push_str(
        "          credential_helper='!f() { echo username=caniko; echo \"password=$HOMEBREW_TAP_TOKEN\"; }; f'\n",
    );
    workflow.push_str("          rm -rf tap\n");
    workflow.push_str("          git -c credential.helper=\"$credential_helper\" clone \"$HOMEBREW_TAP_URL\" tap\n");
    workflow.push_str("          cd tap\n");
    workflow.push_str("          git config credential.helper \"$credential_helper\"\n");
    workflow.push_str("          git config user.email 'ci@modde.tartanoglu.com'\n");
    workflow.push_str("          git config user.name 'modde release bot'\n");
    workflow.push_str("          git remote set-head origin -a\n");
    workflow.push_str(
        "          DEFAULT_BRANCH=\"$(git symbolic-ref --short refs/remotes/origin/HEAD | sed 's|^origin/||')\"\n",
    );
    workflow.push_str("          git checkout \"$DEFAULT_BRANCH\"\n");
    workflow.push_str("          cd ..\n\n");
    workflow.push_str("          nix run '.#rs-harbor' -- brew bump \\\n");
    workflow.push_str("            --name ");
    workflow.push_str(&shell_word(&opts.name));
    workflow.push_str(" \\\n");
    workflow.push_str("            --version \"$VERSION\" \\\n");
    workflow.push_str("            --description ");
    workflow.push_str(&shell_quote(&opts.description));
    workflow.push_str(" \\\n");
    workflow.push_str("            --homepage ");
    workflow.push_str(&shell_quote(&opts.homepage));
    workflow.push_str(" \\\n");
    workflow.push_str("            --license ");
    workflow.push_str(&shell_word(&opts.license));
    workflow.push_str(" \\\n");
    for archive in &archives {
        workflow.push_str("            --archive \"");
        workflow.push_str(archive.key);
        workflow.push('=');
        workflow.push_str("https://codeberg.org/");
        workflow.push_str(&opts.download_repo);
        workflow.push_str("/releases/download/${VERSION}/");
        workflow.push_str(&archive.file_name);
        workflow.push_str(",release/");
        workflow.push_str(&archive.file_name);
        workflow.push_str("\" \\\n");
    }
    for binary in binaries {
        workflow.push_str("            --binary ");
        workflow.push_str(&shell_word(binary));
        workflow.push_str(" \\\n");
    }
    workflow.push_str("            --tap \"$PWD/tap\"\n\n");
    workflow.push_str("          cd tap\n");
    workflow.push_str("          if [ -z \"$(git status --porcelain -- Formula/");
    workflow.push_str(&opts.name);
    workflow.push_str(".rb)\" ]; then\n");
    workflow.push_str("            echo \"tap already contains ");
    workflow.push_str(&opts.name);
    workflow.push_str(" ${VERSION}; nothing to push\"\n");
    workflow.push_str("            exit 0\n");
    workflow.push_str("          fi\n\n");
    workflow.push_str("          git add Formula/");
    workflow.push_str(&opts.name);
    workflow.push_str(".rb\n");
    workflow.push_str("          git commit -m \"");
    workflow.push_str(&opts.name);
    workflow.push_str(" ${VERSION}\"\n");
    workflow.push_str("          git push origin \"HEAD:${DEFAULT_BRANCH}\"\n");
}

struct HomebrewArchive {
    key: &'static str,
    file_name: String,
}

fn homebrew_archives(opts: &HomebrewOptions) -> Vec<HomebrewArchive> {
    [
        ("darwin_arm", "aarch64", "darwin", opts.platforms.darwin_arm),
        (
            "darwin_intel",
            "x86_64",
            "darwin",
            opts.platforms.darwin_intel,
        ),
        ("linux_arm", "aarch64", "linux", opts.platforms.linux_arm),
        ("linux_intel", "x86_64", "linux", opts.platforms.linux_intel),
    ]
    .into_iter()
    .filter(|(_, _, _, enabled)| *enabled)
    .map(|(key, arch, os, _)| HomebrewArchive {
        key,
        file_name: resolve_archive(opts, arch, os),
    })
    .collect()
}

fn resolve_archive(opts: &HomebrewOptions, arch: &str, os: &str) -> String {
    opts.archive_pattern
        .replace("{name}", &opts.name)
        .replace("{version}", "${VERSION}")
        .replace("{arch}", arch)
        .replace("{os}", os)
}

fn homebrew_tap_repo(tap_url: &str) -> String {
    tap_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches(".git")
        .split_once('/')
        .map(|(_, path)| path.to_owned())
        .unwrap_or_else(|| tap_url.to_owned())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_word(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        value.to_owned()
    } else {
        shell_quote(value)
    }
}

fn deny_toml() -> String {
    r#"[advisories]
version = 2

[licenses]
version = 2
allow = ["Apache-2.0", "MIT", "Unicode-3.0", "Unlicense"]

[bans]
multiple-versions = "warn"
wildcards = "allow"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
"#
    .to_owned()
}

fn push_container(workflow: &mut String, platform: Platform, runtime: Runtime) {
    if platform == Platform::Forgejo && runtime == Runtime::Cargo {
        workflow.push_str("    container: rust:alpine\n");
    }
}

fn push_checkout_step(workflow: &mut String, platform: Platform, runtime: Runtime) {
    if platform == Platform::Forgejo && runtime == Runtime::Cargo {
        workflow.push_str(
            r#"      - name: Install Alpine tools
        run: apk add --no-cache git build-base

      - name: Checkout
        run: |
          repo="${GITHUB_REPOSITORY:-${FORGE_REPOSITORY:-}}"
          server="${GITHUB_SERVER_URL:-${FORGE_SERVER_URL:-https://codeberg.org}}"
          sha="${GITHUB_SHA:-${FORGE_SHA:-}}"
          ref="${GITHUB_REF:-${FORGE_REF:-}}"
          if [ -z "$repo" ]; then
            echo "Repository name is unavailable" >&2
            exit 1
          fi
          git init .
          git remote add origin "$server/$repo.git"
          if [ -n "$ref" ]; then
            git fetch --depth=1 origin "$ref"
          else
            git fetch --depth=1 origin "$sha"
          fi
          git checkout --detach FETCH_HEAD

"#,
        );
    } else {
        workflow.push_str("      - name: Checkout\n");
        workflow.push_str("        uses: actions/checkout@v4\n\n");
    }
}

fn push_rust_setup_step(workflow: &mut String, platform: Platform) {
    match platform {
        Platform::Forgejo => {
            workflow.push_str("      - name: Install Rust components\n");
            workflow.push_str("        run: rustup component add clippy rustfmt\n\n");
        }
        Platform::Github => {
            workflow.push_str("      - name: Install Rust\n");
            workflow.push_str("        uses: dtolnay/rust-toolchain@stable\n");
            workflow.push_str("        with:\n");
            workflow.push_str("          toolchain: stable\n");
            workflow.push_str("          components: rustfmt, clippy\n\n");
        }
    }
}

fn push_test_steps(workflow: &mut String, package: &Package, options: &CiOptions) {
    if options.with_nextest {
        workflow.push_str("      - name: Install nextest\n");
        workflow.push_str("        run: cargo install cargo-nextest --locked\n\n");
        workflow.push_str("      - name: Test all features\n");
        workflow.push_str("        run: cargo nextest run --all-features\n\n");
    } else {
        workflow.push_str("      - name: Test all features\n");
        workflow.push_str("        run: cargo test --all-features\n\n");
    }
    if has_features(package) {
        workflow.push_str("      - name: Test no default features\n");
        if options.with_nextest {
            workflow.push_str("        run: cargo nextest run --no-default-features\n\n");
        } else {
            workflow.push_str("        run: cargo test --no-default-features\n\n");
        }
    }
}

fn push_clippy_steps(workflow: &mut String, package: &Package) {
    workflow.push_str("      - name: Clippy all features\n");
    workflow
        .push_str("        run: cargo clippy --all-targets --all-features -- --deny warnings\n\n");
    if has_features(package) {
        workflow.push_str("      - name: Clippy no default features\n");
        workflow.push_str(
            "        run: cargo clippy --all-targets --no-default-features -- --deny warnings\n\n",
        );
    }
}

fn push_quality_tool_install_steps(workflow: &mut String, runtime: Runtime, options: &CiOptions) {
    let prefix = command_prefix(runtime);
    if options.with_audit {
        workflow.push_str("      - name: Install cargo-audit\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo install cargo-audit --locked\n\n");
    }
    if options.with_deny {
        workflow.push_str("      - name: Install cargo-deny\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo install cargo-deny --locked\n\n");
    }
}

fn push_optional_ci_steps(
    workflow: &mut String,
    runtime: Runtime,
    package: &Package,
    options: &CiOptions,
) {
    let prefix = command_prefix(runtime);
    if options.with_msrv {
        workflow.push_str("      - name: Check MSRV\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo +");
        workflow.push_str(package.rust_version.as_deref().expect("validated MSRV"));
        workflow.push_str(" check --all-targets\n\n");
    }
    if options.with_audit {
        workflow.push_str("      - name: Audit dependencies\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo audit\n\n");
    }
    if options.with_deny {
        workflow.push_str("      - name: Deny dependency policy\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo deny check\n\n");
    }
    if options.with_docs {
        workflow.push_str("      - name: Build docs\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo doc --no-deps --all-features\n\n");
    }
}

fn push_optional_publish_steps(workflow: &mut String, runtime: Runtime, options: &CiOptions) {
    let prefix = command_prefix(runtime);
    if options.with_audit {
        workflow.push_str("      - name: Audit dependencies\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo audit\n\n");
    }
    if options.with_deny {
        workflow.push_str("      - name: Deny dependency policy\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo deny check\n\n");
    }
    if options.with_docs {
        workflow.push_str("      - name: Build docs\n");
        workflow.push_str("        run: ");
        workflow.push_str(prefix);
        workflow.push_str("cargo doc --no-deps --all-features\n\n");
    }
}

fn command_prefix(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Cargo => "",
        Runtime::Nix => "nix develop -c ",
    }
}

fn has_features(package: &Package) -> bool {
    !package.features.is_empty()
}

fn push_self_check_steps(
    workflow: &mut String,
    platform: Platform,
    runtime: Runtime,
    runner_override: Option<&str>,
    options: &CiOptions,
) {
    workflow.push_str("      - name: Check generated CI\n");
    workflow.push_str("        run: ");
    workflow.push_str(command_prefix(runtime));
    workflow.push_str("cargo run -- init-ci --platform ");
    workflow.push_str(platform.as_str());
    push_self_check_suffix(workflow, runtime, runner_override, options);
    workflow.push_str("      - name: Check generated flake and hooks\n");
    workflow.push_str("        run: ");
    workflow.push_str(command_prefix(runtime));
    workflow.push_str("cargo run -- init-flake --check\n\n");
}

fn push_self_check_suffix(
    workflow: &mut String,
    runtime: Runtime,
    runner_override: Option<&str>,
    options: &CiOptions,
) {
    if runtime == Runtime::Nix {
        workflow.push_str(" --runtime nix");
    }
    if let Some(runner) = runner_override {
        workflow.push_str(" --runner ");
        workflow.push_str(runner);
    }
    if options.with_nextest {
        workflow.push_str(" --with-nextest");
    }
    if options.with_msrv {
        workflow.push_str(" --with-msrv");
    }
    if options.with_audit {
        workflow.push_str(" --with-audit");
    }
    if options.with_deny {
        workflow.push_str(" --with-deny");
    }
    if options.with_docs {
        workflow.push_str(" --with-docs");
    }
    if options.with_artifacts {
        workflow.push_str(" --with-artifacts");
    }
    workflow.push_str(" --check\n\n");
}

fn runner(platform: Platform, runner_override: Option<&str>) -> String {
    if let Some(runner) = runner_override {
        return runner.to_owned();
    }

    match platform {
        Platform::Forgejo => "codeberg-small".to_owned(),
        Platform::Github => "ubuntu-latest".to_owned(),
    }
}

fn validate_tag_step(cargo_metadata_command: &str) -> String {
    format!(
        r#"      - name: Validate tag
        run: |
          tag="${{GITHUB_REF_NAME:-${{FORGE_REF_NAME:-}}}}"
          if [ -z "$tag" ]; then
            ref="${{GITHUB_REF:-${{FORGE_REF:-}}}}"
            tag="${{ref#refs/tags/}}"
          fi

          if ! printf '%s\n' "$tag" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
            echo "Tag must be an exact semver version like 0.1.1, got '$tag'" >&2
            exit 1
          fi

          version="$({cargo_metadata_command} | grep -m1 -o '"version":"[^"]*"' | cut -d '"' -f4)"
          if [ -z "$version" ]; then
            echo "Could not read package version from cargo metadata" >&2
            exit 1
          fi
          if [ "$tag" != "$version" ]; then
            echo "Tag $tag does not match Cargo.toml package version $version" >&2
            exit 1
          fi

"#
    )
}

fn publish_step(command: &str) -> String {
    format!(
        r#"      - name: Publish
        env:
          CRATES_IO_API_TOKEN: ${{{{ secrets.CRATES_IO_API_TOKEN }}}}
        run: |
          if [ -z "${{CRATES_IO_API_TOKEN:-}}" ] && [ -z "${{CARGO_REGISTRY_TOKEN:-}}" ]; then
            echo "CRATES_IO_API_TOKEN is required to publish to crates.io" >&2
            exit 1
          fi
          export CARGO_REGISTRY_TOKEN="${{CARGO_REGISTRY_TOKEN:-$CRATES_IO_API_TOKEN}}"
          {command}
"#
    )
}
