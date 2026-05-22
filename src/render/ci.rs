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
    pub chocolatey: Option<ChocolateyOptions>,
    pub scoop: Option<ScoopOptions>,
}

#[derive(Debug, Clone)]
pub struct HomebrewOptions {
    /// Formula name, for example `modde`. UpperCamelCased for the Ruby class.
    pub name: String,
    /// Binaries to install. Defaults to `name` if empty.
    pub binaries: Vec<String>,
    /// Tap repo URL, for example `<https://codeberg.org/caniko/homebrew-modde.git>`.
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

#[derive(Debug, Clone)]
pub struct ChocolateyOptions {
    pub name: String,
    pub id: String,
    pub title: String,
    pub authors: Option<String>,
    pub description: String,
    pub project_url: String,
    pub license_url: Option<String>,
    pub tags: Option<String>,
    pub release_notes_url: Option<String>,
    pub download_repo: String,
    pub archive_pattern: String,
    pub push_source: String,
}

#[derive(Debug, Clone)]
pub struct ScoopOptions {
    pub name: String,
    pub bucket_url: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
    pub download_repo: String,
    pub archive_pattern: String,
    pub binaries: Vec<String>,
    pub x64: bool,
    pub arm64: bool,
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
    windows_runner_override: Option<&str>,
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
                windows_runner_override,
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
            content: artifacts_workflow(
                platform,
                runtime,
                package,
                runner_override,
                windows_runner_override,
                &options,
            ),
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
    windows_runner_override: Option<&str>,
    options: CiOptions,
) -> String {
    let mut workflow = String::new();
    workflow.push_str("name: CI\n\n");
    workflow.push_str("on:\n");
    workflow.push_str("  push:\n");
    workflow.push_str("    branches: [trunk]\n");
    workflow.push_str("  pull_request:\n\n");
    push_concurrency(&mut workflow);
    workflow.push_str("jobs:\n");
    workflow.push_str("  test:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(platform, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime, package);
    workflow.push_str("    steps:\n");
    push_checkout_step(&mut workflow, platform);

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
                push_self_check_steps(
                    &mut workflow,
                    platform,
                    runtime,
                    runner_override,
                    windows_runner_override,
                    &options,
                );
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
                push_self_check_steps(
                    &mut workflow,
                    platform,
                    runtime,
                    runner_override,
                    windows_runner_override,
                    &options,
                );
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
    push_concurrency(&mut workflow);
    workflow.push_str("jobs:\n");
    workflow.push_str("  publish:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(platform, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime, package);
    workflow.push_str("    steps:\n");
    push_checkout_step(&mut workflow, platform);

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
    package: &Package,
    runner_override: Option<&str>,
    windows_runner_override: Option<&str>,
    options: &CiOptions,
) -> String {
    let mut workflow = String::new();
    workflow.push_str("name: Release Artifacts\n\n");
    workflow.push_str("on:\n");
    workflow.push_str("  push:\n");
    workflow.push_str("    tags:\n");
    workflow.push_str("      - \"*.*.*\"\n\n");
    push_concurrency(&mut workflow);
    workflow.push_str("jobs:\n");
    let has_windows_packagers = options.chocolatey.is_some() || options.scoop.is_some();
    let linux_job_name = if has_windows_packagers {
        "build-linux"
    } else {
        "build"
    };
    workflow.push_str("  ");
    workflow.push_str(linux_job_name);
    workflow.push_str(":\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(platform, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime, package);
    workflow.push_str("    steps:\n");
    push_checkout_step(&mut workflow, platform);
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
    if has_windows_packagers {
        let windows_runner =
            windows_runner_override.unwrap_or_else(|| default_windows_runner(platform));
        push_windows_build_job(&mut workflow, platform, package, windows_runner, options);
        push_windows_publish_job(&mut workflow, platform, windows_runner, options);
    }
    workflow
}

fn push_windows_build_job(
    workflow: &mut String,
    platform: Platform,
    package: &Package,
    windows_runner: &str,
    options: &CiOptions,
) {
    let matrix = windows_matrix(options);
    workflow.push_str("\n  build-windows:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(windows_runner);
    workflow.push('\n');
    workflow.push_str("    strategy:\n");
    workflow.push_str("      fail-fast: false\n");
    workflow.push_str("      matrix:\n");
    workflow.push_str("        include:\n");
    for row in &matrix {
        workflow.push_str("          - arch: ");
        workflow.push_str(row.arch);
        workflow.push('\n');
        workflow.push_str("            target: ");
        workflow.push_str(row.target);
        workflow.push('\n');
    }
    workflow.push_str("    steps:\n");
    push_checkout_step(workflow, platform);
    push_windows_rust_setup_step(workflow, platform);
    workflow.push_str("      - name: Install Windows target\n");
    workflow.push_str("        run: rustup target add ${{ matrix.target }}\n\n");
    workflow.push_str("      - name: Build release binary\n");
    workflow
        .push_str("        run: cargo build --release --locked --target ${{ matrix.target }}\n\n");
    push_windows_archive_step(workflow, package, options);
    workflow.push_str("      - name: Upload Windows archives\n");
    push_action_uses(workflow, platform, "upload-artifact", "v4");
    workflow.push_str("        with:\n");
    workflow.push_str("          name: windows-${{ matrix.arch }}\n");
    workflow.push_str("          path: release/*.zip\n");
}

fn push_windows_publish_job(
    workflow: &mut String,
    platform: Platform,
    windows_runner: &str,
    options: &CiOptions,
) {
    workflow.push_str("\n  publish-windows-packages:\n");
    workflow.push_str("    needs: build-windows\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(windows_runner);
    workflow.push('\n');
    workflow.push_str("    steps:\n");
    push_checkout_step(workflow, platform);
    workflow.push_str("      - name: Download Windows archives\n");
    push_action_uses(workflow, platform, "download-artifact", "v4");
    workflow.push_str("        with:\n");
    workflow.push_str("          pattern: windows-*\n");
    workflow.push_str("          path: release\n");
    workflow.push_str("          merge-multiple: true\n\n");
    push_windows_rust_setup_step(workflow, platform);
    workflow.push_str("      - name: Install simit\n");
    workflow.push_str("        run: |\n");
    workflow.push_str("          # TODO(cache): cache cargo install output for tagged releases.\n");
    workflow.push_str("          cargo install --locked simit\n\n");

    if let Some(chocolatey) = &options.chocolatey {
        push_chocolatey_publish_step(workflow, chocolatey);
    }
    if let Some(scoop) = &options.scoop {
        push_scoop_publish_step(workflow, scoop);
    }
}

#[derive(Clone, Copy)]
struct WindowsMatrixRow {
    arch: &'static str,
    target: &'static str,
}

fn windows_matrix(options: &CiOptions) -> Vec<WindowsMatrixRow> {
    let mut rows = Vec::new();
    if options.chocolatey.is_some() || options.scoop.as_ref().is_some_and(|scoop| scoop.x64) {
        rows.push(WindowsMatrixRow {
            arch: "x64",
            target: "x86_64-pc-windows-msvc",
        });
    }
    if options.scoop.as_ref().is_some_and(|scoop| scoop.arm64) {
        rows.push(WindowsMatrixRow {
            arch: "arm64",
            target: "aarch64-pc-windows-msvc",
        });
    }
    rows
}

fn push_windows_rust_setup_step(workflow: &mut String, platform: Platform) {
    match platform {
        Platform::Github => {
            workflow.push_str("      - name: Install Rust\n");
            workflow.push_str("        uses: dtolnay/rust-toolchain@stable\n");
            workflow.push_str("        with:\n");
            workflow.push_str("          toolchain: stable\n\n");
        }
        Platform::Forgejo => {
            workflow.push_str("      - name: Install Rust\n");
            workflow.push_str("        run: |\n");
            workflow.push_str("          rustup toolchain install stable --profile minimal\n");
            workflow.push_str("          rustup default stable\n\n");
        }
    }
}

fn push_windows_archive_step(workflow: &mut String, package: &Package, options: &CiOptions) {
    let archives = windows_archive_specs(options);
    workflow.push_str("      - name: Package Windows archive\n");
    workflow.push_str("        shell: pwsh\n");
    workflow.push_str("        run: |\n");
    workflow.push_str("          $version = $env:GITHUB_REF_NAME\n");
    workflow.push_str("          if (-not $version) { $version = $env:FORGE_REF_NAME }\n");
    workflow.push_str("          if (-not $version -and $env:GITHUB_REF) { $version = $env:GITHUB_REF -replace '^refs/tags/', '' }\n");
    workflow.push_str("          if (-not $version -and $env:FORGE_REF) { $version = $env:FORGE_REF -replace '^refs/tags/', '' }\n");
    workflow.push_str("          if (-not $version) { throw 'Release tag name is unavailable' }\n");
    workflow.push_str("          New-Item -ItemType Directory -Force release | Out-Null\n");
    workflow.push_str("          $binary = \"target/${{ matrix.target }}/release/");
    workflow.push_str(&package.name);
    workflow.push_str(".exe\"\n");
    workflow.push_str(
        "          if (-not (Test-Path $binary)) { throw \"missing Windows binary: $binary\" }\n",
    );
    for archive in archives {
        workflow.push_str("          if (\"${{ matrix.arch }}\" -eq \"");
        workflow.push_str(archive.arch);
        workflow.push_str("\") {\n");
        workflow.push_str("            $archive = Join-Path release ");
        workflow.push_str(&ps_expanding_double_quote(&archive.file_name));
        workflow.push('\n');
        workflow.push_str(
            "            Compress-Archive -Path $binary -DestinationPath $archive -Force\n",
        );
        workflow.push_str("          }\n");
    }
    workflow.push('\n');
}

#[derive(Clone, Eq, PartialEq)]
struct WindowsArchiveSpec {
    arch: &'static str,
    key: &'static str,
    file_name: String,
}

fn windows_archive_specs(options: &CiOptions) -> Vec<WindowsArchiveSpec> {
    let mut specs = Vec::new();
    if let Some(chocolatey) = &options.chocolatey {
        specs.push(WindowsArchiveSpec {
            arch: "x64",
            key: "x64",
            file_name: resolve_windows_archive(
                &chocolatey.archive_pattern,
                &chocolatey.name,
                "x86_64",
            ),
        });
    }
    if let Some(scoop) = &options.scoop {
        if scoop.x64 {
            specs.push(WindowsArchiveSpec {
                arch: "x64",
                key: "x64",
                file_name: resolve_windows_archive(&scoop.archive_pattern, &scoop.name, "x86_64"),
            });
        }
        if scoop.arm64 {
            specs.push(WindowsArchiveSpec {
                arch: "arm64",
                key: "arm64",
                file_name: resolve_windows_archive(&scoop.archive_pattern, &scoop.name, "aarch64"),
            });
        }
    }
    specs.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    specs.dedup_by(|left, right| left.file_name == right.file_name && left.arch == right.arch);
    specs
}

fn resolve_windows_archive(pattern: &str, name: &str, arch: &str) -> String {
    pattern
        .replace("{name}", name)
        .replace("{version}", "$version")
        .replace("{arch}", arch)
}

fn push_chocolatey_publish_step(workflow: &mut String, opts: &ChocolateyOptions) {
    let archive = resolve_windows_archive(&opts.archive_pattern, &opts.name, "x86_64");
    workflow.push_str("      - name: Install Chocolatey\n");
    workflow.push_str("        shell: pwsh\n");
    workflow.push_str("        run: |\n");
    workflow.push_str("          if (Get-Command choco -ErrorAction SilentlyContinue) { choco --version; exit 0 }\n");
    workflow.push_str("          Set-ExecutionPolicy Bypass -Scope Process -Force\n");
    workflow.push_str("          [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072\n");
    workflow.push_str(
        "          iwr https://community.chocolatey.org/install.ps1 -UseBasicParsing | iex\n\n",
    );
    workflow.push_str("      - name: Publish Chocolatey package\n");
    workflow.push_str("        env:\n");
    workflow.push_str("          CHOCOLATEY_API_KEY: ${{ secrets.chocolatey_api_key }}\n");
    workflow.push_str("          CHOCO_PUSH_SOURCE: ");
    workflow.push_str(&opts.push_source);
    workflow.push('\n');
    workflow.push_str("        shell: pwsh\n");
    workflow.push_str("        run: |\n");
    workflow.push_str("          if (-not $env:CHOCOLATEY_API_KEY) { Write-Host 'CHOCOLATEY_API_KEY not configured; skipping Chocolatey package update.'; exit 0 }\n");
    push_windows_version_lines(workflow);
    workflow.push_str("          simit chocolatey bump `\n");
    workflow.push_str("            --version $version `\n");
    workflow.push_str("            --package-dir chocolatey-package `\n");
    workflow.push_str("            --archive ");
    workflow.push_str(&ps_expanding_double_quote(&format!(
        "x64=release/{archive}"
    )));
    workflow.push_str(" `\n");
    push_chocolatey_cli_flags(workflow, opts);
    workflow.push_str("            --push `\n");
    workflow.push_str("            --push-source \"$env:CHOCO_PUSH_SOURCE\" `\n");
    workflow.push_str("            --api-key-env CHOCOLATEY_API_KEY\n\n");
}

fn push_scoop_publish_step(workflow: &mut String, opts: &ScoopOptions) {
    workflow.push_str("      - name: Publish Scoop bucket\n");
    workflow.push_str("        env:\n");
    workflow.push_str("          SCOOP_BUCKET_TOKEN: ${{ secrets.scoop_bucket_token }}\n");
    workflow.push_str("          SCOOP_BUCKET_URL: ");
    workflow.push_str(&opts.bucket_url);
    workflow.push('\n');
    workflow.push_str("        shell: pwsh\n");
    workflow.push_str("        run: |\n");
    workflow.push_str("          if (-not $env:SCOOP_BUCKET_TOKEN) { Write-Host 'SCOOP_BUCKET_TOKEN not configured; skipping Scoop bucket update.'; exit 0 }\n");
    push_windows_version_lines(workflow);
    workflow.push_str("          $credentialHelper = '!f() { echo username=caniko; echo \"password=$SCOOP_BUCKET_TOKEN\"; }; f'\n");
    workflow.push_str("          if (Test-Path bucket) { Remove-Item -Recurse -Force bucket }\n");
    workflow.push_str("          git -c credential.helper=\"$credentialHelper\" clone \"$env:SCOOP_BUCKET_URL\" bucket\n");
    workflow.push_str("          Push-Location bucket\n");
    workflow.push_str("          git config credential.helper \"$credentialHelper\"\n");
    workflow.push_str("          git config user.email 'ci@simit.rs'\n");
    workflow.push_str("          git config user.name 'simit release bot'\n");
    workflow.push_str("          git remote set-head origin -a\n");
    workflow.push_str("          $defaultBranch = (git symbolic-ref --short refs/remotes/origin/HEAD) -replace '^origin/', ''\n");
    workflow.push_str("          git checkout $defaultBranch\n");
    workflow.push_str("          Pop-Location\n");
    workflow.push_str("          simit scoop bump `\n");
    workflow.push_str("            --version $version `\n");
    workflow.push_str("            --bucket bucket `\n");
    for spec in scoop_archive_specs(opts) {
        workflow.push_str("            --archive ");
        workflow.push_str(&ps_expanding_double_quote(&format!(
            "{}=release/{}",
            spec.key, spec.file_name
        )));
        workflow.push_str(" `\n");
    }
    push_scoop_cli_flags(workflow, opts);
    workflow.push_str("            --push\n\n");
}

fn push_windows_version_lines(workflow: &mut String) {
    workflow.push_str("          $version = $env:GITHUB_REF_NAME\n");
    workflow.push_str("          if (-not $version) { $version = $env:FORGE_REF_NAME }\n");
    workflow.push_str("          if (-not $version -and $env:GITHUB_REF) { $version = $env:GITHUB_REF -replace '^refs/tags/', '' }\n");
    workflow.push_str("          if (-not $version -and $env:FORGE_REF) { $version = $env:FORGE_REF -replace '^refs/tags/', '' }\n");
    workflow.push_str("          if (-not $version) { throw 'Release tag name is unavailable' }\n");
}

fn push_chocolatey_cli_flags(workflow: &mut String, opts: &ChocolateyOptions) {
    push_ps_arg(workflow, "--choco-name", &opts.name);
    push_ps_arg(workflow, "--choco-id", &opts.id);
    push_ps_arg(workflow, "--choco-title", &opts.title);
    if let Some(authors) = &opts.authors {
        push_ps_arg(workflow, "--choco-authors", authors);
    }
    push_ps_arg(workflow, "--choco-description", &opts.description);
    push_ps_arg(workflow, "--choco-project-url", &opts.project_url);
    if let Some(license_url) = &opts.license_url {
        push_ps_arg(workflow, "--choco-license-url", license_url);
    }
    if let Some(tags) = &opts.tags {
        push_ps_arg(workflow, "--choco-tags", tags);
    }
    if let Some(release_notes_url) = &opts.release_notes_url {
        push_ps_arg(workflow, "--choco-release-notes-url", release_notes_url);
    }
    push_ps_arg(workflow, "--choco-download-repo", &opts.download_repo);
    push_ps_arg(workflow, "--choco-archive-pattern", &opts.archive_pattern);
}

fn push_scoop_cli_flags(workflow: &mut String, opts: &ScoopOptions) {
    push_ps_arg(workflow, "--scoop-name", &opts.name);
    push_ps_arg(workflow, "--scoop-bucket", &opts.bucket_url);
    push_ps_arg(workflow, "--scoop-description", &opts.description);
    push_ps_arg(workflow, "--scoop-homepage", &opts.homepage);
    push_ps_arg(workflow, "--scoop-license", &opts.license);
    push_ps_arg(workflow, "--scoop-download-repo", &opts.download_repo);
    push_ps_arg(workflow, "--scoop-archive-pattern", &opts.archive_pattern);
    let binaries = if opts.binaries.is_empty() {
        vec![opts.name.as_str()]
    } else {
        opts.binaries.iter().map(String::as_str).collect::<Vec<_>>()
    };
    for binary in binaries {
        push_ps_arg(workflow, "--scoop-binary", binary);
    }
    if !opts.x64 {
        push_ps_arg(workflow, "--scoop-no-arch", "x64");
    }
    if !opts.arm64 {
        push_ps_arg(workflow, "--scoop-no-arch", "arm64");
    }
}

fn scoop_archive_specs(opts: &ScoopOptions) -> Vec<WindowsArchiveSpec> {
    let mut specs = Vec::new();
    if opts.x64 {
        specs.push(WindowsArchiveSpec {
            arch: "x64",
            key: "x64",
            file_name: resolve_windows_archive(&opts.archive_pattern, &opts.name, "x86_64"),
        });
    }
    if opts.arm64 {
        specs.push(WindowsArchiveSpec {
            arch: "arm64",
            key: "arm64",
            file_name: resolve_windows_archive(&opts.archive_pattern, &opts.name, "aarch64"),
        });
    }
    specs
}

fn push_ps_arg(workflow: &mut String, flag: &str, value: &str) {
    workflow.push_str("            ");
    workflow.push_str(flag);
    workflow.push(' ');
    workflow.push_str(&ps_double_quote(value));
    workflow.push_str(" `\n");
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

fn ps_double_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('`', "``")
            .replace('"', "`\"")
            .replace('$', "`$")
    )
}

fn ps_expanding_double_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('`', "``").replace('"', "`\""))
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

fn push_concurrency(workflow: &mut String) {
    workflow.push_str("concurrency:\n");
    workflow.push_str("  group: ${{ github.workflow }}-${{ github.ref }}\n");
    workflow.push_str("  cancel-in-progress: true\n\n");
}

fn push_container(workflow: &mut String, platform: Platform, runtime: Runtime, package: &Package) {
    if platform == Platform::Forgejo && runtime == Runtime::Cargo {
        workflow.push_str("    container: ");
        workflow.push_str(&rust_container(package));
        workflow.push('\n');
    }
}

fn push_checkout_step(workflow: &mut String, platform: Platform) {
    workflow.push_str("      - name: Checkout\n");
    push_action_uses(workflow, platform, "checkout", "v4");
    workflow.push('\n');
}

fn push_action_uses(workflow: &mut String, platform: Platform, action: &str, version: &str) {
    if platform == Platform::Forgejo {
        workflow.push_str("        uses: https://code.forgejo.org/actions/");
        workflow.push_str(action);
        workflow.push('@');
        workflow.push_str(version);
        workflow.push('\n');
    } else {
        workflow.push_str("        uses: actions/");
        workflow.push_str(action);
        workflow.push('@');
        workflow.push_str(version);
        workflow.push('\n');
    }
}

fn rust_container(package: &Package) -> String {
    let Some(rust_version) = package.rust_version.as_deref() else {
        return "rust:bookworm".to_owned();
    };
    let distro = if rust_version_supports_trixie(rust_version) {
        "trixie"
    } else {
        "bookworm"
    };
    format!("rust:{rust_version}-{distro}")
}

fn rust_version_supports_trixie(rust_version: &str) -> bool {
    let mut parts = rust_version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u64>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u64>().ok());
    matches!(
        (major, minor),
        (Some(major), _) if major > 1
    ) || matches!((major, minor), (Some(1), Some(minor)) if minor >= 93)
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
    windows_runner_override: Option<&str>,
    options: &CiOptions,
) {
    workflow.push_str("      - name: Check generated CI\n");
    workflow.push_str("        run: ");
    workflow.push_str(command_prefix(runtime));
    workflow.push_str("cargo run -- init-ci --platform ");
    workflow.push_str(platform.as_str());
    push_self_check_suffix(
        workflow,
        runtime,
        runner_override,
        windows_runner_override,
        options,
    );
    workflow.push_str("      - name: Check generated flake and hooks\n");
    workflow.push_str("        run: ");
    workflow.push_str(command_prefix(runtime));
    workflow.push_str("cargo run -- init-flake --check\n\n");
}

fn push_self_check_suffix(
    workflow: &mut String,
    runtime: Runtime,
    runner_override: Option<&str>,
    windows_runner_override: Option<&str>,
    options: &CiOptions,
) {
    if runtime == Runtime::Nix {
        workflow.push_str(" --runtime nix");
    }
    if let Some(runner) = runner_override {
        workflow.push_str(" --runner ");
        workflow.push_str(runner);
    }
    if let Some(runner) = windows_runner_override {
        workflow.push_str(" --windows-runner ");
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
    if options.homebrew.is_some() {
        workflow.push_str(" --with-homebrew");
    }
    if options.chocolatey.is_some() {
        workflow.push_str(" --with-chocolatey");
    }
    if options.scoop.is_some() {
        workflow.push_str(" --with-scoop");
    }
    workflow.push_str(" --check\n\n");
}

fn runner(platform: Platform, runner_override: Option<&str>) -> String {
    if let Some(runner) = runner_override {
        return runner.to_owned();
    }

    match platform {
        Platform::Forgejo => "atlas".to_owned(),
        Platform::Github => "ubuntu-latest".to_owned(),
    }
}

fn default_windows_runner(platform: Platform) -> &'static str {
    match platform {
        Platform::Github => "windows-latest",
        Platform::Forgejo => "windows-runner",
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
