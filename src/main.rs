use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{collections::BTreeMap, fmt};

use anyhow::{Context, Result, anyhow, bail};
use camino::Utf8PathBuf;
use semver::Version;
use serde::Deserialize;
use toml_edit::DocumentMut;

fn main() {
    if let Err(err) = run(env::args_os().skip(1).collect()) {
        eprintln!("simit: {err:#}");
        std::process::exit(1);
    }
}

fn run(args: Vec<OsString>) -> Result<()> {
    let command = parse_args(args)?;
    match command {
        CommandSpec::Commit(commit) => commit.run(),
        CommandSpec::InitCi(init_ci) => init_ci.run(),
    }
}

#[derive(Debug)]
enum CommandSpec {
    Commit(CommitCommand),
    InitCi(InitCiCommand),
}

#[derive(Debug)]
struct CommitCommand {
    bump: Bump,
    package: Option<String>,
    create_tag: bool,
    sign_tag: bool,
    git_args: Vec<OsString>,
}

impl CommitCommand {
    fn run(self) -> Result<()> {
        let start = env::current_dir().context("reading current directory")?;
        let manifest = find_manifest(&start)?;
        let metadata = cargo_metadata(&manifest)?;
        let package = select_package(&metadata, self.package.as_deref())?;
        let old_version = Version::parse(&package.version)
            .with_context(|| format!("parsing version {}", package.version))?;
        let new_version = bump_version(old_version, self.bump);
        let workspace_root = metadata.workspace_root.as_std_path();
        let manifest_path = package.manifest_path.as_std_path();

        if self.create_tag {
            ensure_tag_absent(workspace_root, &new_version)?;
        }

        update_manifest_version(manifest_path, &new_version)?;
        let lock_path = metadata.workspace_root.join("Cargo.lock");
        if lock_path.exists() {
            cargo_update(workspace_root, &package.name, &new_version)?;
        }

        stage_version_files(workspace_root, manifest_path, lock_path.exists())?;
        git_commit(workspace_root, &self.git_args)?;

        if self.create_tag {
            git_tag(workspace_root, &new_version, self.sign_tag)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
struct InitCiCommand {
    platform: Platform,
    check: bool,
    runner: Option<String>,
    runtime: RuntimeChoice,
}

impl InitCiCommand {
    fn run(self) -> Result<()> {
        let start = env::current_dir().context("reading current directory")?;
        let manifest = find_manifest(&start)?;
        let metadata = cargo_metadata(&manifest)?;
        let package = select_package(&metadata, None)?;
        let workspace_root = metadata.workspace_root.as_std_path();
        let runtime = self.runtime.resolve(workspace_root)?;
        let self_check = metadata
            .packages
            .iter()
            .any(|package| package.name == "simit");
        let workflows = generate_workflows(
            self.platform,
            runtime,
            package,
            self_check,
            self.runner.as_deref(),
        );

        if self.check {
            check_workflows(workspace_root, &workflows)
        } else {
            write_workflows(workspace_root, &workflows)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bump {
    Patch,
    Minor,
    Major,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Forgejo,
    Github,
}

impl Platform {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "forgejo" => Ok(Self::Forgejo),
            "github" => Ok(Self::Github),
            _ => bail!("--platform must be one of: forgejo, github"),
        }
    }

    fn workflow_dir(self) -> &'static str {
        match self {
            Self::Forgejo => ".forgejo/workflows",
            Self::Github => ".github/workflows",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Forgejo => "forgejo",
            Self::Github => "github",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeChoice {
    Auto,
    Cargo,
    Nix,
}

impl RuntimeChoice {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "cargo" => Ok(Self::Cargo),
            "nix" => Ok(Self::Nix),
            _ => bail!("--runtime must be one of: auto, cargo, nix"),
        }
    }

    fn resolve(self, workspace_root: &Path) -> Result<Runtime> {
        match self {
            Self::Auto | Self::Cargo => Ok(Runtime::Cargo),
            Self::Nix => {
                if !workspace_root.join("flake.nix").exists() {
                    bail!("--runtime nix requires flake.nix at the workspace root");
                }
                Ok(Runtime::Nix)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Runtime {
    Nix,
    Cargo,
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nix => f.write_str("nix"),
            Self::Cargo => f.write_str("cargo"),
        }
    }
}

#[derive(Debug)]
struct GeneratedWorkflow {
    relative_path: PathBuf,
    content: String,
}

fn parse_args(args: Vec<OsString>) -> Result<CommandSpec> {
    let mut iter = args.into_iter();
    let Some(command) = iter.next() else {
        bail!("expected command: simit commit|init-ci");
    };

    if command == "commit" {
        return parse_commit_args(iter.collect());
    }

    if command == "init-ci" {
        return parse_init_ci_args(iter.collect());
    }

    bail!(
        "unknown command {:?}; expected `commit` or `init-ci`",
        command
    );
}

fn parse_commit_args(args: Vec<OsString>) -> Result<CommandSpec> {
    let mut iter = args.into_iter();
    let mut package = None;
    let mut create_tag = true;
    let mut sign_tag = true;
    let mut bump = None;
    let mut git_args = Vec::new();

    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--no-tag") if bump.is_none() => create_tag = false,
            Some("--no-sign") if bump.is_none() => sign_tag = false,
            Some("--package") if bump.is_none() => {
                let Some(name) = iter.next() else {
                    bail!("--package requires a package name");
                };
                package = Some(
                    name.into_string()
                        .map_err(|_| anyhow!("--package value must be UTF-8"))?,
                );
            }
            Some("patch") if bump.is_none() => {
                bump = Some(Bump::Patch);
                git_args.extend(iter);
                break;
            }
            Some("minor") if bump.is_none() => {
                bump = Some(Bump::Minor);
                git_args.extend(iter);
                break;
            }
            Some("major") if bump.is_none() => {
                bump = Some(Bump::Major);
                git_args.extend(iter);
                break;
            }
            _ if bump.is_none() => {
                bail!("expected patch, minor, or major before git commit arguments");
            }
            _ => unreachable!("remaining git arguments are collected after bump"),
        }
    }

    let Some(bump) = bump else {
        bail!("expected patch, minor, or major");
    };

    Ok(CommandSpec::Commit(CommitCommand {
        bump,
        package,
        create_tag,
        sign_tag,
        git_args,
    }))
}

fn parse_init_ci_args(args: Vec<OsString>) -> Result<CommandSpec> {
    let mut iter = args.into_iter();
    let mut platform = None;
    let mut check = false;
    let mut runner = None;
    let mut runtime = RuntimeChoice::Auto;

    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--platform") => {
                let Some(value) = iter.next() else {
                    bail!("--platform requires a value");
                };
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow!("--platform value must be UTF-8"))?;
                platform = Some(Platform::parse(value)?);
            }
            Some("--runner") => {
                let Some(value) = iter.next() else {
                    bail!("--runner requires a value");
                };
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow!("--runner value must be UTF-8"))?;
                validate_runner(value)?;
                runner = Some(value.to_owned());
            }
            Some("--runtime") => {
                let Some(value) = iter.next() else {
                    bail!("--runtime requires a value");
                };
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow!("--runtime value must be UTF-8"))?;
                runtime = RuntimeChoice::parse(value)?;
            }
            Some("--check") => check = true,
            _ => {
                bail!(
                    "usage: simit init-ci --platform forgejo|github [--runtime auto|cargo|nix] [--runner <label>] [--check]"
                )
            }
        }
    }

    let Some(platform) = platform else {
        bail!("init-ci requires --platform forgejo|github");
    };

    Ok(CommandSpec::InitCi(InitCiCommand {
        platform,
        check,
        runner,
        runtime,
    }))
}

fn find_manifest(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not find Cargo.toml in {} or its parents",
        start.display()
    );
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: Utf8PathBuf,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    rust_version: Option<String>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
    manifest_path: Utf8PathBuf,
}

fn cargo_metadata(manifest: &Path) -> Result<Metadata> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .context("running cargo metadata")?;

    if !output.status.success() {
        bail!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout).context("parsing cargo metadata")
}

fn select_package<'a>(metadata: &'a Metadata, requested: Option<&str>) -> Result<&'a Package> {
    let workspace_packages = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .collect::<Vec<_>>();

    if let Some(name) = requested {
        return workspace_packages
            .into_iter()
            .find(|package| package.name == name)
            .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"));
    }

    match workspace_packages.as_slice() {
        [package] => Ok(package),
        [] => bail!("workspace has no packages"),
        _ => bail!("workspace has multiple packages; rerun with --package <name>"),
    }
}

fn bump_version(mut version: Version, bump: Bump) -> Version {
    version.pre = semver::Prerelease::EMPTY;
    version.build = semver::BuildMetadata::EMPTY;

    match bump {
        Bump::Patch => version.patch += 1,
        Bump::Minor => {
            version.minor += 1;
            version.patch = 0;
        }
        Bump::Major => {
            version.major += 1;
            version.minor = 0;
            version.patch = 0;
        }
    }

    version
}

fn generate_workflows(
    platform: Platform,
    runtime: Runtime,
    package: &Package,
    self_check: bool,
    runner_override: Option<&str>,
) -> Vec<GeneratedWorkflow> {
    let dir = PathBuf::from(platform.workflow_dir());
    vec![
        GeneratedWorkflow {
            relative_path: dir.join("ci.yaml"),
            content: ci_workflow(platform, runtime, package, self_check, runner_override),
        },
        GeneratedWorkflow {
            relative_path: dir.join("publish-crate.yaml"),
            content: publish_workflow(platform, runtime, package, runner_override),
        },
    ]
}

fn write_workflows(workspace_root: &Path, workflows: &[GeneratedWorkflow]) -> Result<()> {
    for workflow in workflows {
        let path = workspace_root.join(&workflow.relative_path);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("workflow path has no parent: {}", path.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        fs::write(&path, &workflow.content)
            .with_context(|| format!("writing {}", path.display()))?;
    }

    Ok(())
}

fn check_workflows(workspace_root: &Path, workflows: &[GeneratedWorkflow]) -> Result<()> {
    let mut mismatches = Vec::new();

    for workflow in workflows {
        let path = workspace_root.join(&workflow.relative_path);
        match fs::read_to_string(&path) {
            Ok(actual) if actual == workflow.content => {}
            Ok(_) => mismatches.push(format!("{} differs", workflow.relative_path.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                mismatches.push(format!("{} is missing", workflow.relative_path.display()));
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        bail!(
            "CI workflows are not up to date; run `simit init-ci --platform {}`:\n{}",
            infer_platform_name(workflows),
            mismatches.join("\n")
        );
    }
}

fn infer_platform_name(workflows: &[GeneratedWorkflow]) -> &'static str {
    workflows
        .first()
        .and_then(|workflow| workflow.relative_path.components().next())
        .and_then(|component| component.as_os_str().to_str())
        .map(|dir| {
            if dir == ".github" {
                "github"
            } else {
                "forgejo"
            }
        })
        .unwrap_or("forgejo")
}

fn ci_workflow(
    platform: Platform,
    runtime: Runtime,
    package: &Package,
    self_check: bool,
    runner_override: Option<&str>,
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
    workflow.push_str(&runner(platform, runtime, JobKind::Ci, runner_override));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime, package);
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
            if self_check {
                workflow.push_str("      - name: Check generated CI\n");
                workflow.push_str("        run: nix develop -c cargo run -- init-ci --platform ");
                workflow.push_str(platform.as_str());
                push_self_check_suffix(&mut workflow, runtime, runner_override);
            }
            workflow.push_str("      - name: Clippy\n");
            workflow.push_str(
                "        run: nix develop -c cargo clippy --all-targets -- --deny warnings\n\n",
            );
            workflow.push_str("      - name: Package crate\n");
            workflow.push_str("        run: nix develop -c cargo package --allow-dirty\n");
        }
        Runtime::Cargo => {
            push_rust_setup_step(&mut workflow, platform, package);
            push_test_steps(&mut workflow, package);
            if self_check {
                workflow.push_str("      - name: Check generated CI\n");
                workflow.push_str("        run: cargo run -- init-ci --platform ");
                workflow.push_str(platform.as_str());
                push_self_check_suffix(&mut workflow, runtime, runner_override);
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
) -> String {
    let mut workflow = String::new();
    workflow.push_str("name: Publish Crate\n\n");
    workflow.push_str("on:\n");
    workflow.push_str("  push:\n");
    workflow.push_str("    tags:\n");
    workflow.push_str("      - \"*.*.*\"\n\n");
    workflow.push_str("jobs:\n");
    workflow.push_str("  publish:\n");
    workflow.push_str("    runs-on: ");
    workflow.push_str(&runner(
        platform,
        runtime,
        JobKind::Publish,
        runner_override,
    ));
    workflow.push('\n');
    push_container(&mut workflow, platform, runtime, package);
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
            workflow.push_str("      - name: Clippy\n");
            workflow.push_str(
                "        run: nix develop -c cargo clippy --all-targets -- --deny warnings\n\n",
            );
            workflow.push_str("      - name: Dry-run publish\n");
            workflow.push_str("        run: nix develop -c cargo publish --dry-run\n\n");
            workflow.push_str(&publish_step("nix develop -c cargo publish"));
        }
        Runtime::Cargo => {
            push_rust_setup_step(&mut workflow, platform, package);
            workflow.push_str(&validate_tag_step(
                "cargo metadata --no-deps --format-version 1",
            ));
            push_test_steps(&mut workflow, package);
            push_clippy_steps(&mut workflow, package);
            workflow.push_str("      - name: Dry-run publish\n");
            workflow.push_str("        run: cargo publish --dry-run\n\n");
            workflow.push_str(&publish_step("cargo publish"));
        }
    }

    workflow
}

fn rust_toolchain_action(platform: Platform) -> &'static str {
    match platform {
        Platform::Forgejo => "https://github.com/dtolnay/rust-toolchain@stable",
        Platform::Github => "dtolnay/rust-toolchain@stable",
    }
}

fn push_container(workflow: &mut String, platform: Platform, runtime: Runtime, package: &Package) {
    if platform == Platform::Forgejo && runtime == Runtime::Cargo {
        workflow.push_str("    container: ");
        workflow.push_str(&rust_container_image(package));
        workflow.push('\n');
    }
}

fn rust_container_image(package: &Package) -> String {
    match package.rust_version.as_deref() {
        Some(version) => format!("rust:{version}-bookworm"),
        None => "rust:stable-bookworm".to_owned(),
    }
}

fn push_checkout_step(workflow: &mut String, platform: Platform, runtime: Runtime) {
    if platform == Platform::Forgejo && runtime == Runtime::Cargo {
        workflow.push_str(
            r#"      - name: Checkout
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

fn push_rust_setup_step(workflow: &mut String, platform: Platform, package: &Package) {
    match platform {
        Platform::Forgejo => {
            workflow.push_str("      - name: Install Rust components\n");
            workflow.push_str("        run: rustup component add clippy rustfmt\n\n");
        }
        Platform::Github => {
            workflow.push_str("      - name: Install Rust\n");
            workflow.push_str("        uses: ");
            workflow.push_str(rust_toolchain_action(platform));
            workflow.push('\n');
            workflow.push_str("        with:\n");
            workflow.push_str("          toolchain: ");
            workflow.push_str(package.rust_version.as_deref().unwrap_or("stable"));
            workflow.push('\n');
            workflow.push_str("          components: rustfmt, clippy\n\n");
        }
    }
}

fn push_test_steps(workflow: &mut String, package: &Package) {
    workflow.push_str("      - name: Test all features\n");
    workflow.push_str("        run: cargo test --all-features\n\n");
    if has_features(package) {
        workflow.push_str("      - name: Test no default features\n");
        workflow.push_str("        run: cargo test --no-default-features\n\n");
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

fn has_features(package: &Package) -> bool {
    !package.features.is_empty()
}

fn validate_runner(value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("--runner cannot be empty");
    }

    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("--runner may only contain ASCII letters, digits, '.', '_', and '-'");
    }

    Ok(())
}

fn push_self_check_suffix(workflow: &mut String, runtime: Runtime, runner_override: Option<&str>) {
    if runtime == Runtime::Nix {
        workflow.push_str(" --runtime nix");
    }
    if let Some(runner) = runner_override {
        workflow.push_str(" --runner ");
        workflow.push_str(runner);
    }
    workflow.push_str(" --check\n\n");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Ci,
    Publish,
}

fn runner(
    platform: Platform,
    runtime: Runtime,
    _job_kind: JobKind,
    runner_override: Option<&str>,
) -> String {
    if let Some(runner) = runner_override {
        return runner.to_owned();
    }

    match (platform, runtime) {
        (Platform::Forgejo, Runtime::Cargo) => "codeberg-small".to_owned(),
        (Platform::Forgejo, Runtime::Nix) => "codeberg-small".to_owned(),
        (Platform::Github, _) => "ubuntu-latest".to_owned(),
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

fn update_manifest_version(manifest_path: &Path, version: &Version) -> Result<()> {
    let original = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut document = original
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let package = document
        .get_mut("package")
        .and_then(|item| item.as_table_mut())
        .ok_or_else(|| anyhow!("{} has no [package] table", manifest_path.display()))?;

    let version_item = package
        .get_mut("version")
        .ok_or_else(|| anyhow!("{} has no package.version", manifest_path.display()))?;

    if version_item.as_str().is_none() {
        bail!(
            "{} package.version must be a literal string for simit to update it",
            manifest_path.display()
        );
    }

    *version_item = toml_edit::value(version.to_string());
    fs::write(manifest_path, document.to_string())
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    Ok(())
}

fn cargo_update(workspace_root: &Path, package: &str, version: &Version) -> Result<()> {
    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args(["update", "-p", package, "--precise"])
        .arg(version.to_string())
        .status()
        .context("running cargo update")?;

    if !status.success() {
        bail!("cargo update failed while updating Cargo.lock");
    }

    Ok(())
}

fn stage_version_files(workspace_root: &Path, manifest_path: &Path, has_lock: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(workspace_root).args(["add", "--"]);
    command.arg(manifest_path);
    if has_lock {
        command.arg(workspace_root.join("Cargo.lock"));
    }

    let status = command.status().context("staging version files")?;
    if !status.success() {
        bail!("git add failed while staging version files");
    }

    Ok(())
}

fn git_commit(workspace_root: &Path, git_args: &[OsString]) -> Result<()> {
    let status = Command::new("git")
        .current_dir(workspace_root)
        .arg("commit")
        .args(git_args)
        .status()
        .context("running git commit")?;

    if !status.success() {
        bail!("git commit failed");
    }

    Ok(())
}

fn ensure_tag_absent(workspace_root: &Path, version: &Version) -> Result<()> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/tags/{version}"))
        .output()
        .context("checking existing git tag")?;

    if output.status.success() {
        bail!("tag {version} already exists");
    }

    Ok(())
}

fn git_tag(workspace_root: &Path, version: &Version, sign_tag: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(workspace_root).arg("tag");

    if sign_tag {
        command
            .arg("-s")
            .arg("-m")
            .arg(format!("Release {version}"));
    } else {
        command.arg("--no-sign");
    }

    let status = command
        .arg(version.to_string())
        .status()
        .context("creating git tag")?;

    if !status.success() {
        bail!("git tag failed for {version}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_patch() {
        assert_eq!(
            bump_version(Version::parse("1.2.3").unwrap(), Bump::Patch).to_string(),
            "1.2.4"
        );
    }

    #[test]
    fn bumps_minor_and_resets_patch() {
        assert_eq!(
            bump_version(Version::parse("1.2.3").unwrap(), Bump::Minor).to_string(),
            "1.3.0"
        );
    }

    #[test]
    fn bumps_major_and_resets_minor_patch() {
        assert_eq!(
            bump_version(Version::parse("1.2.3").unwrap(), Bump::Major).to_string(),
            "2.0.0"
        );
    }

    #[test]
    fn clears_pre_release_and_build_metadata() {
        assert_eq!(
            bump_version(
                Version::parse("1.2.3-alpha.1+build.7").unwrap(),
                Bump::Patch
            )
            .to_string(),
            "1.2.4"
        );
    }
}
