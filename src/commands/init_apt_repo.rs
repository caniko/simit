use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::InitAptRepoCommand;
use crate::commands::scaffold::{
    ArtifactCheck, CheckPrintMode, WriteArtifact, bootstrap_repo, prepare_target, print_next_steps,
};
use crate::config::ProjectConfig;
use crate::registry::{self, FeatureStatus};
use crate::render::apt_conf;

const WORKFLOW_PATH: &str = ".forgejo/workflows/pages.yml";
const README_PATH: &str = "README.md";
const CONF_PATH: &str = "conf/distributions";
const DISTS_KEEP_PATH: &str = "dists/.gitkeep";
const POOL_KEEP_PATH: &str = "pool/.gitkeep";

pub fn run(command: InitAptRepoCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init apt-repo", command.check, command.print, command.diff)?;
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::representative_package(&metadata, command.package.as_deref())?;
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_apt(command.apt.as_overrides(), &package)?;
    if resolved.pages_provider != "codeberg-git-pages" {
        bail!(
            "unsupported apt Pages provider `{}`; only codeberg-git-pages is implemented",
            resolved.pages_provider
        );
    }
    let public_url = resolved
        .public_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("[apt].public_url is required by `simit init apt-repo`"))?;
    let source_public_key = workspace_root.join("dist/apt/key.gpg.asc");
    let public_key = std::fs::read_to_string(&source_public_key).with_context(|| {
        format!(
            "reading {}; required producer: export the apt signing public key before bootstrapping the repository; validation: test -s {}",
            source_public_key.display(),
            source_public_key.display()
        )
    })?;
    if public_key.trim().is_empty() {
        bail!(
            "{} is empty; required producer: export the apt signing public key; validation: test -s {}",
            source_public_key.display(),
            source_public_key.display()
        );
    }
    let runner = resolved.pages_runner.as_deref().unwrap_or("atlas");
    let workflow = render_workflow(public_url, runner, &resolved.branch);
    let readme = render_readme(
        public_url,
        &resolved.repo_url,
        &resolved.branch,
        &resolved.distribution,
        &resolved.components,
    );
    let distributions = apt_conf::render_distributions(&resolved);
    let target = command.target.as_std_path();

    match mode {
        CheckPrintMode::Print => {
            print!("{workflow}");
            Ok(())
        }
        CheckPrintMode::Check { diff } => {
            ArtifactCheck {
                label: "apt Pages workflow",
                path: &target.join(WORKFLOW_PATH),
                expected: &workflow,
                remediation: &format!("run `simit init apt-repo --target {}`", target.display()),
            }
            .verify(diff)?;
            ArtifactCheck {
                label: "apt distributions config",
                path: &target.join(CONF_PATH),
                expected: &distributions,
                remediation: &format!("run `simit init apt-repo --target {}`", target.display()),
            }
            .verify(diff)?;
            ArtifactCheck {
                label: "apt repository README",
                path: &target.join(README_PATH),
                expected: &readme,
                remediation: &format!("run `simit init apt-repo --target {}`", target.display()),
            }
            .verify(diff)?;
            ArtifactCheck {
                label: "apt repository public key",
                path: &target.join("key.gpg.asc"),
                expected: &public_key,
                remediation: &format!("run `simit init apt-repo --target {}`", target.display()),
            }
            .verify(diff)?;
            ArtifactCheck {
                label: "apt repository dists placeholder",
                path: &target.join(DISTS_KEEP_PATH),
                expected: "",
                remediation: &format!("run `simit init apt-repo --target {}`", target.display()),
            }
            .verify(diff)?;
            ArtifactCheck {
                label: "apt repository pool placeholder",
                path: &target.join(POOL_KEEP_PATH),
                expected: "",
                remediation: &format!("run `simit init apt-repo --target {}`", target.display()),
            }
            .verify(diff)
        }
        CheckPrintMode::Write => {
            prepare_target(target, command.no_git)?;
            WriteArtifact {
                path: &target.join(WORKFLOW_PATH),
                contents: &workflow,
            }
            .commit()?;
            WriteArtifact {
                path: &target.join(README_PATH),
                contents: &readme,
            }
            .commit()?;
            WriteArtifact {
                path: &target.join(CONF_PATH),
                contents: &distributions,
            }
            .commit()?;
            WriteArtifact {
                path: &target.join("key.gpg.asc"),
                contents: &public_key,
            }
            .commit()?;
            WriteArtifact {
                path: &target.join(DISTS_KEEP_PATH),
                contents: "",
            }
            .commit()?;
            WriteArtifact {
                path: &target.join(POOL_KEEP_PATH),
                contents: "",
            }
            .commit()?;
            if !command.no_git {
                bootstrap_repo(target, &resolved.repo_url, &resolved.branch, README_PATH)?;
                crate::commands::scaffold::run_git(
                    target,
                    &[
                        "add",
                        "--",
                        WORKFLOW_PATH,
                        CONF_PATH,
                        "key.gpg.asc",
                        DISTS_KEEP_PATH,
                        POOL_KEEP_PATH,
                    ],
                )?;
            }
            print_next_steps(
                target,
                "Initialised apt repository Pages site",
                &[
                    format!(
                        "git -C {} commit -m 'Initial apt repository'",
                        target.display()
                    ),
                    format!(
                        "git -C {} push -u origin {}",
                        target.display(),
                        resolved.branch
                    ),
                ],
            );
            registry::touch_current_project_or_warn([("apt", FeatureStatus::Managed)]);
            Ok(())
        }
    }
}

fn render_workflow(public_url: &str, runner: &str, branch: &str) -> String {
    let checkout = crate::render::ci::forgejo_action_ref("checkout", "v4.3.1");
    format!(
        "---\n# Generated by simit. Manual edits will be reported as ci=drift.\nname: Publish apt repository Pages site\n\non:\n  push:\n    branches: [{branch}]\n  workflow_dispatch:\n\npermissions:\n  contents: read\n\njobs:\n  pages:\n    runs-on: {runner}\n    steps:\n      - uses: {checkout}\n      - name: Assemble site\n        run: |\n          set -euo pipefail\n          mkdir -p _site/dists _site/pool\n          cp -a dists/. _site/dists/\n          cp -a pool/. _site/pool/\n          test -s key.gpg.asc\n          cp key.gpg.asc _site/\n      - name: Deploy Codeberg Pages\n        uses: https://codeberg.org/git-pages/action@v2\n        with:\n          site: {public_url}\n          server: codeberg.page\n          token: ${{{{ forge.token }}}}\n          source: _site/\n",
        checkout = checkout,
        public_url = public_url.trim_end_matches('/'),
    )
}

fn render_readme(
    public_url: &str,
    remote: &str,
    branch: &str,
    distribution: &str,
    components: &str,
) -> String {
    format!(
        "# APT repository\n\nThis repository is generated by [simit](https://codeberg.org/caniko/simit) and served at <{public_url}>.\n\nThe `{branch}` branch contains the reprepro database and public package files. Release automation publishes signed packages here; the Pages workflow deploys `dists/`, `pool/`, and `key.gpg.asc`.\n\nConfigure Debian clients with:\n\n```text\ndeb [signed-by=/etc/apt/keyrings/apt-repository.gpg] {public_url} {distribution} {components}\n```\n\nRepository remote: `{remote}`\n",
        public_url = public_url.trim_end_matches('/'),
    )
}
