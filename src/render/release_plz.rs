//! Release-plz owns crate releases; Simit qualifies its immutable source first.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::ci::{self, CiOptions};
use crate::{
    cargo::{Metadata, Package},
    cli::{CiProvider, Platform, Runtime},
    config::ProjectConfig,
    project::GeneratedFile,
};

pub const VERSION: &str = "0.3.172";
pub const REVISION: &str = "8b523fa6c4eb4992f18365ac50a196b4dc6a1013";
// GNU asset for ubuntu-24.04; musl's stack detection rejects changelog parsing.
pub const CLI_SHA256: &str = "84b7013d164e1fc780b31dc9ef190977830599d6cb5ce81c993f6fcbd3cdc1aa";
pub const CONFIG: &str = "release-plz.toml";
pub const HELPER: &str = ".simit/release-plz.py";

pub fn validate_backend(provider: CiProvider, platform: Platform) -> Result<()> {
    if provider != CiProvider::Actions || platform != Platform::Github {
        bail!("release-plz publication requires GitHub Actions");
    }
    Ok(())
}

pub fn files(
    metadata: &Metadata,
    config: &ProjectConfig,
    runtime: Runtime,
    options: &CiOptions,
) -> Result<Vec<GeneratedFile>> {
    let packages: Vec<Package> = metadata
        .packages
        .iter()
        .filter(|p| metadata.workspace_members.contains(&p.id))
        .cloned()
        .collect();
    let publishable: Vec<_> = packages
        .iter()
        .filter(|p| p.is_publishable_to_crates_io())
        .collect();
    if publishable.is_empty() {
        bail!("release-plz requires at least one package publishable to crates.io");
    }
    if packages
        .iter()
        .any(|p| p.is_publishable() && !p.is_publishable_to_crates_io())
    {
        bail!("release-plz currently supports only the crates.io registry");
    }
    let owns_hosted_release =
        config.release.github.is_some() || options.with_artifacts || config.prebuild.is_some();
    if publishable.len() > 1 && owns_hosted_release {
        bail!(
            "release-plz multi-package publication cannot be combined with repository-wide binary release workflows"
        );
    }
    if !config.release.signing.required {
        bail!(
            "release-plz requires [release.signing].required = true and an authorized CI signing key"
        );
    }
    let trust_root = Path::new(&config.release.signing.trust_root);
    if trust_root.as_os_str().is_empty()
        || !trust_root
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        || config.release.signing.trust_root.contains('\\')
    {
        bail!("release-plz signing trust_root must be a committed relative path");
    }
    let credentials = &config.ci.release_plz;
    if credentials.app_id_variable.is_some() != credentials.app_private_key_secret.is_some() {
        bail!("[ci.release_plz] requires app_id_variable and app_private_key_secret together");
    }
    if credentials.app_id_variable.is_none()
        && credentials
            .token_secret
            .eq_ignore_ascii_case("GITHUB_TOKEN")
    {
        bail!(
            "release-plz requires an App token or PAT; GITHUB_TOKEN cannot trigger release-PR checks or downstream tag workflows"
        );
    }
    for name in [
        &credentials.token_secret,
        &credentials.registry_token_secret,
        &credentials.signing_key_secret,
        &credentials.signing_passphrase_secret,
    ]
    .into_iter()
    .chain(credentials.app_id_variable.iter())
    .chain(credentials.app_private_key_secret.iter())
    {
        crate::config::validate_environment_name("[ci.release_plz] credential name", name)?;
    }
    ci::validate_credential_free_workflow("", options)?;
    // Keep graph validation shared with the coordinated publisher. release-plz
    // supplies the actual dependency ordering and per-package version updates.
    crate::commands::release_plan::build_release_plan(metadata, &[])?;
    let tag = if publishable.len() == 1 {
        format!("{}{{{{ version }}}}", config.release.tag_prefix.as_str())
    } else {
        "{{ package }}/v{{ version }}".into()
    };
    let mut toml = format!(
        "{}\n# release-plz {VERSION}, reviewed upstream revision {REVISION}\n[workspace]\nrelease_always = false\nrelease = false\ngit_release_enable = {}\ngit_tag_name = {:?}\npr_branch_prefix = \"release-plz-\"\n",
        ci::GENERATED_WORKFLOW_MARKER,
        !owns_hosted_release,
        tag
    );
    for package in &publishable {
        toml.push_str(&format!(
            "\n[[package]]\nname = {:?}\nrelease = true\n",
            package.name
        ));
    }
    Ok(vec![
        GeneratedFile {
            relative_path: CONFIG.into(),
            content: toml,
        },
        GeneratedFile {
            relative_path: HELPER.into(),
            content: include_str!("ci/release_plz.py").into(),
        },
        GeneratedFile {
            relative_path: ".github/workflows/release-plz.yaml".into(),
            content: workflow(config, runtime, options, &packages),
        },
    ])
}

/// Migration must never silently activate two independent crate publishers.
pub fn validate_migration(
    root: &Path,
    config: &ProjectConfig,
    files: &[GeneratedFile],
    obsolete: &[PathBuf],
) -> Result<()> {
    for file in files {
        if super::workflow_templates::is_template_output(
            &config.ci,
            &file.relative_path,
            &file.content,
        ) && competing_publisher(root, &file.content, &mut BTreeSet::new())?
        {
            bail!(
                "release-plz migration: reconcile competing publisher {} before enabling release-plz",
                file.relative_path.display()
            );
        }
    }
    for path in [
        CONFIG,
        ".release-plz.toml",
        ".config/release-plz.toml",
        HELPER,
    ] {
        match fs::read_to_string(root.join(path)) {
            Ok(content) if !ci::is_generated_workflow_marker(&content) => {
                bail!(
                    "release-plz migration: reconcile hand-written {path} before enabling generated release-plz"
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading {path} for release-plz migration"));
            }
        }
    }
    for directory in [".github/workflows", ".forgejo/workflows"] {
        let path = root.join(directory);
        if !path.is_dir() {
            continue;
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if !matches!(
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str()),
                Some("yaml" | "yml")
            ) {
                continue;
            }
            let content = fs::read_to_string(entry.path())?;
            let relative = Path::new(directory).join(entry.file_name());
            let replaced = obsolete.contains(&relative)
                || files.iter().any(|file| {
                    super::workflow_templates::same_output(root, &file.relative_path, &relative)
                });
            if (!ci::is_generated_workflow_marker(&content)
                || !replaced
                || super::workflow_templates::is_template_output(&config.ci, &relative, &content))
                && competing_publisher(root, &content, &mut BTreeSet::new())?
            {
                bail!(
                    "release-plz migration: reconcile competing publisher {} before enabling release-plz",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

fn competing_publisher(
    root: &Path,
    content: &str,
    seen: &mut BTreeSet<(PathBuf, PathBuf, Option<bool>)>,
) -> Result<bool> {
    let workflow: serde_yaml::Value = serde_yaml::from_str(content)?;
    let Some(jobs) = workflow.get("jobs").and_then(serde_yaml::Value::as_mapping) else {
        return Ok(false);
    };
    for job in jobs.values() {
        if let Some(local) = job.get("uses").and_then(serde_yaml::Value::as_str)
            && local.starts_with("./")
            && local_publisher(root, root, local, seen, true, None)?
        {
            return Ok(true);
        }
        for step in job
            .get("steps")
            .and_then(serde_yaml::Value::as_sequence)
            .into_iter()
            .flatten()
        {
            if let Some(action) = step.get("uses").and_then(serde_yaml::Value::as_str)
                && (action.contains("release-plz/action@")
                    || action.contains("/publish-crates@")
                    || action.starts_with("./"))
            {
                return Ok(true);
            }
            if let Some(action) = step.get("uses").and_then(serde_yaml::Value::as_str)
                && action
                    .split('@')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("actions-rs/cargo"))
            {
                // The action passes its required command input directly to Cargo.
                let command = step
                    .get("with")
                    .and_then(|inputs| inputs.get("command"))
                    .and_then(serde_yaml::Value::as_str)
                    .context(
                        "release-plz migration: reconcile opaque Cargo-action publisher command",
                    )?;
                if command.contains(['$', '{', '}']) {
                    bail!("release-plz migration: reconcile opaque Cargo-action publisher command");
                }
                if commands_publish(root, root, &format!("cargo {command}"), seen, true)? {
                    return Ok(true);
                }
            }
            if let Some(run) = step.get("run").and_then(serde_yaml::Value::as_str) {
                let directory = step
                    .get("working-directory")
                    .or_else(|| job.get("defaults")?.get("run")?.get("working-directory"))
                    .or_else(|| {
                        workflow
                            .get("defaults")?
                            .get("run")?
                            .get("working-directory")
                    })
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or(".");
                let shell = step
                    .get("shell")
                    .or_else(|| job.get("defaults")?.get("run")?.get("shell"))
                    .or_else(|| workflow.get("defaults")?.get("run")?.get("shell"))
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or("");
                let shell_program = Path::new(shell.split_whitespace().next().unwrap_or(""))
                    .file_stem()
                    .and_then(|name| name.to_str());
                let is_shell = !matches!(
                    shell_program,
                    Some("python" | "python3" | "node" | "pwsh" | "ruby")
                );
                if commands_publish(root, &root.join(directory), shell, seen, true)?
                    || commands_publish(root, &root.join(directory), run, seen, is_shell)?
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn local_publisher(
    root: &Path,
    cwd: &Path,
    local: &str,
    seen: &mut BTreeSet<(PathBuf, PathBuf, Option<bool>)>,
    workflow: bool,
    shell: Option<bool>,
) -> Result<bool> {
    let root = root.canonicalize()?;
    let cwd = cwd.canonicalize().with_context(|| {
        format!(
            "release-plz migration: reconcile opaque publisher directory {}",
            cwd.display()
        )
    })?;
    let path = cwd.join(local).canonicalize().with_context(|| {
        format!("release-plz migration: reconcile uninspectable publisher reference {local}")
    })?;
    // Ignore untracked build outputs, not tracked sources matching ignore rules.
    // A local build must not change migration's source inventory.
    if !workflow
        && path.starts_with(&root)
        && std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["check-ignore", "--quiet", "--"])
            .arg(&path)
            .output()
            .is_ok_and(|output| output.status.success())
    {
        return Ok(false);
    }
    if !path.starts_with(&root)
        || !path.is_file()
        || fs::metadata(&path)?.len() > 1024 * 1024
        || seen.len() >= 128
    {
        bail!("release-plz migration: reconcile uninspectable publisher reference {local}");
    }
    let content = fs::read_to_string(&path).with_context(|| {
        format!("release-plz migration: reconcile opaque publisher reference {local}")
    })?;
    // Direct execution uses the shebang. An explicit interpreter/source call
    // overrides it, and each resulting grammar has its own recursion receipt.
    let shell = if let Some(shell) = shell {
        shell
    } else {
        let shebang = content
            .lines()
            .next()
            .unwrap_or("")
            .strip_prefix("#!")
            .unwrap_or("");
        let words = shlex::split(shebang)
            .context("release-plz migration: reconcile opaque publisher shebang")?;
        let mut words = words.iter().map(String::as_str);
        let mut program = words.next().unwrap_or("");
        if Path::new(program)
            .file_name()
            .is_some_and(|name| name == "env")
        {
            let mut options = true;
            loop {
                let word = words.next().unwrap_or("");
                if word.contains(['$', '`']) {
                    bail!("release-plz migration: reconcile computed publisher shebang");
                }
                if !word.starts_with('-') && word.contains('=') {
                    continue;
                }
                if options && word.starts_with('-') {
                    match word {
                        "--" => options = false,
                        "-i"
                        | "--ignore-environment"
                        | "-v"
                        | "--debug"
                        | "-S"
                        | "--split-string" => {}
                        "-u" | "--unset" | "-a" | "--argv0" => {
                            words.next().context(
                                "release-plz migration: missing publisher shebang option value",
                            )?;
                        }
                        word if word.starts_with("--unset=") || word.starts_with("--argv0=") => {}
                        _ => bail!(
                            "release-plz migration: reconcile opaque publisher shebang option {word}"
                        ),
                    }
                    continue;
                }
                program = word;
                break;
            }
        }
        !matches!(
            Path::new(program)
                .file_stem()
                .and_then(|name| name.to_str()),
            Some("python" | "python3" | "node" | "pwsh" | "ruby")
        )
    };
    if !seen.insert((
        cwd.clone(),
        path.clone(),
        if workflow { None } else { Some(shell) },
    )) {
        return Ok(false);
    }
    if workflow {
        competing_publisher(&root, &content, seen)
    } else {
        commands_publish(&root, &cwd, &content, seen, shell)
    }
}

fn commands_publish(
    root: &Path,
    cwd: &Path,
    run: &str,
    seen: &mut BTreeSet<(PathBuf, PathBuf, Option<bool>)>,
    is_shell: bool,
) -> Result<bool> {
    // ponytail: bounded literal references, not general shell/program analysis.
    // Computed/downloaded commands still require operator migration review.
    let run = run
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\\\n", "");
    // Shell evaluation also runs inside assignments and data arguments. Use the
    // grammar's command nodes, not token/quote stripping, so nested commands and
    // unspaced shell operators remain distinct from quoted literal strings.
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_bash::LANGUAGE.into())?;
    let tree = parser
        .parse(&run, None)
        .context("parsing publisher commands")?;
    let mut nodes = if is_shell {
        vec![tree.root_node()]
    } else {
        Vec::new()
    };
    let mut commands = Vec::new();
    while let Some(node) = nodes.pop() {
        if matches!(node.kind(), "command_substitution" | "process_substitution")
            && node.has_error()
        {
            bail!("release-plz migration: reconcile opaque publisher substitution");
        }
        if node.kind() == "command" {
            let mut words = Vec::new();
            for word in node
                .child_by_field_name("name")
                .into_iter()
                .chain(node.children_by_field_name("argument", &mut node.walk()))
            {
                let text = word.utf8_text(run.as_bytes())?;
                // Lex one grammar-delimited word at a time. Nested quotes in
                // assignments must not turn data into a phantom executable.
                let literal = shlex::split(text).filter(|words| words.len() == 1);
                words.push(literal.map_or_else(|| text.to_owned(), |words| words[0].clone()));
            }
            commands.push(words);
        }
        nodes.extend(node.named_children(&mut node.walk()));
    }
    for shell_words in &commands {
        let mut executable = true;
        let mut wrapper = None;
        let mut option_value = false;
        let mut launcher = false;
        for word in shell_words {
            let word = word.as_str();
            if option_value {
                option_value = false;
                continue;
            }
            if executable
                && !word.starts_with('-')
                && let Some((name, _)) = word.split_once('=')
                && (wrapper == Some("env")
                    || crate::config::validate_environment_name("publisher prefix", name).is_ok())
            {
                continue;
            }
            if executable && wrapper.is_some() && word.starts_with('-') {
                match word {
                    "-v" | "-V" if wrapper == Some("command") => break,
                    "-p" if wrapper == Some("command") => {}
                    "-u" | "--unset" | "--user" | "-g" | "--group" | "-a" | "--argv0" => {
                        option_value = true;
                    }
                    "--"
                    | "-i"
                    | "--ignore-environment"
                    | "-0"
                    | "--null"
                    | "--debug"
                    | "-E"
                    | "--preserve-env"
                    | "-H"
                    | "--set-home"
                    | "-n"
                    | "--non-interactive" => {}
                    word if [
                        "--unset=",
                        "--user=",
                        "--group=",
                        "--argv0=",
                        "--preserve-env=",
                    ]
                    .iter()
                    .any(|option| word.starts_with(option)) => {}
                    _ => bail!(
                        "release-plz migration: reconcile opaque publisher wrapper option {word}"
                    ),
                }
                continue;
            }
            let local_executable = executable;
            let program = Path::new(word).file_name().and_then(|name| name.to_str());
            if local_executable && (word.contains(['$', '`']) || program == Some("eval")) {
                bail!("release-plz migration: reconcile opaque publisher executable {word}");
            }
            wrapper = program.filter(|program| {
                local_executable
                    && matches!(*program, "env" | "sudo" | "exec" | "command" | "builtin")
            });
            if local_executable && wrapper.is_none() {
                launcher = matches!(
                    program,
                    Some("nix" | "bash" | "sh" | "python" | "python3" | "node" | "pwsh" | "ruby")
                );
            }
            executable =
                wrapper.is_some() || (launcher && matches!(word, "-c" | "-lc" | "--command"));
            if local_executable
                && !Path::new(word).is_absolute()
                && word.contains('/')
                && !word.contains(['$', '{', '}'])
                && (word.starts_with("./") || cwd.join(word).is_file())
                && local_publisher(root, cwd, word, seen, false, None)?
            {
                return Ok(true);
            }
        }
    }
    let commands: Vec<_> = commands.iter().map(|words| words.join(" ")).collect();
    let lines = std::iter::once(run.as_str())
        .chain(commands.iter().map(String::as_str))
        .flat_map(|fragment| fragment.split(['\n', ';']))
        .map(|line| (line, false));
    // Native argv can span lines. Do not merge unrelated Cargo-looking data
    // across lines when inspecting this additional native-launch-only view.
    for (line, native_only) in lines.chain((!is_shell).then_some((run.as_str(), true))) {
        // The broad token scan also recognizes literal Cargo calls in scripts
        // written in other languages (for example Python subprocess arrays).
        let words: Vec<_> = line
            .split(|c: char| c.is_whitespace() || "\"'[](),;".contains(c))
            .filter(|word| !word.is_empty())
            .collect();
        for (index, word) in words.iter().enumerate() {
            let program = Path::new(word).file_name().and_then(|name| name.to_str());
            let native_launch = !is_shell
                && matches!(
                    word.rsplit('.').next(),
                    Some(
                        "run"
                            | "call"
                            | "check_call"
                            | "check_output"
                            | "Popen"
                            | "system"
                            | "exec"
                            | "execv"
                            | "execve"
                            | "spawn"
                            | "spawnSync"
                            | "execFile"
                            | "execFileSync"
                            | "execSync"
                            | "&"
                    )
                );
            if native_only && !native_launch {
                continue;
            }
            let arguments = &words[index + 1..];
            let keyword_override = |keyword: &str| {
                line.match_indices(keyword).any(|(position, keyword)| {
                    let mut prefix = &line[..position];
                    let mut suffix = line[position + keyword.len()..].trim_start();
                    // Object keys can be quoted; an argv string containing a
                    // key/value pair is still data, not a native-call option.
                    if let Some(quote) = prefix.chars().last().filter(|c| matches!(c, '\'' | '"')) {
                        let Some(rest) = suffix.strip_prefix(quote) else {
                            return false;
                        };
                        prefix = &prefix[..prefix.len() - quote.len_utf8()];
                        suffix = rest.trim_start();
                    }
                    (suffix.starts_with('=')
                        || (keyword != "executable" && suffix.starts_with(':')))
                        && shlex::split(prefix).is_some_and(|prefix| {
                            prefix
                                .last()
                                .is_some_and(|word| word.ends_with([',', '(', '{', ':']))
                        })
                })
            };
            if native_launch && keyword_override("executable") {
                bail!("release-plz migration: reconcile native publisher execution override");
            }
            let arguments = if arguments.first().is_some_and(|arg| arg.starts_with('+')) {
                &arguments[1..]
            } else {
                arguments
            };
            if matches!(program, Some("release-plz" | "cargo-release"))
                || (program == Some("cargo")
                    && (arguments.first() == Some(&"publish")
                        || (arguments.first().is_some_and(|arg| arg.starts_with('-'))
                            && arguments.contains(&"publish"))))
            {
                return Ok(true);
            }
            let sourced = matches!(*word, "source" | ".");
            let arguments = if sourced && arguments.first() == Some(&"--") {
                &arguments[1..]
            } else {
                arguments
            };
            let arguments = match arguments {
                ["args=", rest @ ..] | ["args", "=", rest @ ..] if native_launch => rest,
                _ => arguments,
            };
            if native_launch
                && arguments.first().is_some_and(|local| {
                    (!Path::new(local).is_absolute() && local.contains('/'))
                        || cwd.join(local).is_file()
                })
                && (keyword_override("cwd") || keyword_override("chdir"))
            {
                bail!(
                    "release-plz migration: reconcile native publisher working-directory override"
                );
            }
            if (sourced
                || native_launch
                || matches!(
                    program,
                    Some("bash" | "sh" | "python" | "python3" | "node" | "pwsh" | "ruby")
                ))
                && let Some(local) = arguments
                    .iter()
                    .take(if sourced || native_launch {
                        1
                    } else {
                        arguments.len()
                    })
                    .find(|local| {
                        !local.starts_with('-')
                            && !local.contains(['$', '{', '}'])
                            && ((native_launch && local.starts_with("./"))
                                || cwd.join(local).is_file()
                                || (!Path::new(local).is_absolute()
                                    && matches!(
                                        Path::new(local).extension().and_then(|ext| ext.to_str()),
                                        Some("sh" | "bash" | "py" | "js" | "ps1" | "rb")
                                    )))
                    })
                && local_publisher(
                    root,
                    cwd,
                    local,
                    seen,
                    false,
                    (!native_launch).then_some(sourced || matches!(program, Some("bash" | "sh"))),
                )?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn auxiliary_path(path: &Path) -> bool {
    path == Path::new(CONFIG) || path == Path::new(HELPER)
}

pub fn credential_contract(
    config: &ProjectConfig,
) -> Vec<super::release_workflow::ReleaseCredential> {
    use super::release_workflow::{
        ReleaseCredential, ReleaseCredentialContext, ReleaseCredentialKind, ReleaseCredentialScope,
    };
    let config = &config.ci.release_plz;
    let mut credentials = vec![
        ReleaseCredential {
            name: "SIMIT_PUBLISHER_REVISION".into(),
            kind: ReleaseCredentialKind::Variable,
            scope: ReleaseCredentialScope::Repo,
            channel: "release-plz".into(),
            required: true,
            context: ReleaseCredentialContext::Vars,
            description: Some("Qualified immutable Simit engine revision with release-plz archive admission support.".into()),
        },
        ReleaseCredential::secret(
            &config.registry_token_secret,
            ReleaseCredentialScope::Repo,
            "release-plz",
            true,
            "crates.io API token for release-plz publication.",
        ),
        ReleaseCredential::secret(
            &config.signing_key_secret,
            ReleaseCredentialScope::Repo,
            "release-plz",
            true,
            "OpenPGP private key authorized by the committed release trust root; identity comes from its UID.",
        ),
        ReleaseCredential::secret(
            &config.signing_passphrase_secret,
            ReleaseCredentialScope::Repo,
            "release-plz",
            false,
            "Optional passphrase for the release-plz signing key.",
        ),
    ];
    if let (Some(id), Some(key)) = (&config.app_id_variable, &config.app_private_key_secret) {
        credentials.push(ReleaseCredential { name: id.clone(), kind: ReleaseCredentialKind::Variable, scope: ReleaseCredentialScope::Repo, channel: "release-plz".into(), required: true, context: ReleaseCredentialContext::Vars, description: Some("GitHub App installed on this repository with contents and pull-requests write permission.".into()) });
        credentials.push(ReleaseCredential::secret(
            key,
            ReleaseCredentialScope::Repo,
            "release-plz",
            true,
            "Private key for the repository-scoped GitHub App installation token.",
        ));
    } else {
        credentials.push(ReleaseCredential::secret(&config.token_secret, ReleaseCredentialScope::Repo, "release-plz", true, "Repository-scoped PAT with contents and pull-requests write permission; must trigger PR checks and tag workflows."));
    }
    credentials
}

fn checkout(w: &mut String) {
    w.push_str(&format!("      - uses: {}\n        with:\n          ref: ${{{{ github.sha }}}}\n          fetch-depth: 0\n          persist-credentials: false\n\n", ci::github_action_ref("actions/checkout", "v7.0.1")));
}

fn install(w: &mut String, runtime: Runtime) {
    let prefix = if runtime == Runtime::Nix {
        "nix develop -c "
    } else {
        ""
    };
    w.push_str(&format!(r#"      - name: Install pinned release-plz {VERSION}
        run: |
          set -euo pipefail
          mkdir -p "$RUNNER_TEMP/release-plz/bin"
          archive="$RUNNER_TEMP/release-plz.tar.gz"
          curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --connect-timeout 10 --max-time 120 \
            'https://github.com/release-plz/release-plz/releases/download/release-plz-v{VERSION}/release-plz-x86_64-unknown-linux-gnu.tar.gz' -o "$archive"
          printf '%s  %s\n' '{CLI_SHA256}' "$archive" | sha256sum --check --strict
          tar -xzf "$archive" -C "$RUNNER_TEMP/release-plz/bin" release-plz
          {prefix}"$RUNNER_TEMP/release-plz/bin/release-plz" --version

"#));
}

fn authentication(w: &mut String, config: &ProjectConfig) -> String {
    let credentials = &config.ci.release_plz;
    if let (Some(id), Some(key)) = (
        &credentials.app_id_variable,
        &credentials.app_private_key_secret,
    ) {
        w.push_str(&format!("      - name: Create repository-scoped release token\n        id: app\n        uses: {}\n        with:\n          app-id: ${{{{ vars.{id} }}}}\n          private-key: ${{{{ secrets.{key} }}}}\n          owner: ${{{{ github.repository_owner }}}}\n          repositories: ${{{{ github.event.repository.name }}}}\n          permission-contents: write\n          permission-pull-requests: write\n\n", ci::github_action_ref("actions/create-github-app-token", "v2")));
        "${{ steps.app.outputs.token }}".into()
    } else {
        format!("${{{{ secrets.{} }}}}", credentials.token_secret)
    }
}

fn signing(w: &mut String, config: &ProjectConfig) {
    let credentials = &config.ci.release_plz;
    w.push_str(&format!("      - name: Configure authorized tag signing\n        env:\n          RELEASE_KEY: ${{{{ secrets.{} }}}}\n          RELEASE_PASSPHRASE: ${{{{ secrets.{} }}}}\n          RELEASE_TRUST_ROOT: {:?}\n        run: |\n", credentials.signing_key_secret, credentials.signing_passphrase_secret, config.release.signing.trust_root));
    w.push_str(r#"          set -euo pipefail
          test -n "$RELEASE_KEY" || { echo 'Missing release-plz signing key' >&2; exit 1; }
          export GNUPGHOME="$RUNNER_TEMP/release-plz-gnupg"
          mkdir -m 700 "$GNUPGHOME"
          printf '%s' "$RELEASE_KEY" | gpg --batch --import
          python3 "$RUNNER_TEMP/release-plz-admission.py" identity "$RELEASE_TRUST_ROOT"
          printf '%s' "$RELEASE_PASSPHRASE" > "$GNUPGHOME/passphrase"
          chmod 600 "$GNUPGHOME/passphrase"
          cat > "$GNUPGHOME/gpg-sign" <<'SIMIT_GPG'
          #!/bin/sh
          exec gpg --batch --pinentry-mode loopback --passphrase-file "$GNUPGHOME/passphrase" "$@"
          SIMIT_GPG
          chmod 700 "$GNUPGHOME/gpg-sign"
          git config --local gpg.program "$GNUPGHOME/gpg-sign"
          git config --local tag.gpgSign true
          git config --local commit.gpgSign true
          printf 'release-plz signing probe' | "$GNUPGHOME/gpg-sign" --local-user "$(git config user.signingkey)" --detach-sign > "$GNUPGHOME/probe.sig"
          echo "GNUPGHOME=$GNUPGHOME" >> "$GITHUB_ENV"

"#);
}

fn invocation(runtime: Runtime, command: &str) -> String {
    let prefix = if runtime == Runtime::Nix {
        "nix develop -c "
    } else {
        ""
    };
    format!(
        "{prefix}python3 \"$RUNNER_TEMP/release-plz-admission.py\" run {command} --config \"$RUNNER_TEMP/release-plz.toml\""
    )
}

fn prepare(w: &mut String) {
    w.push_str(
        r#"      - name: Preserve event-owned release policy and publication guard
        run: |
          set -euo pipefail
          cp .simit/release-plz.py "$RUNNER_TEMP/release-plz-admission.py"
          cp release-plz.toml "$RUNNER_TEMP/release-plz.toml"
          git clone --no-hardlinks --no-checkout . "$RUNNER_TEMP/release-plz-event-source"
          mkdir -p "$RUNNER_TEMP/release-plz-guard"
          cat > "$RUNNER_TEMP/release-plz-guard/cargo" <<'SIMIT_CARGO'
          #!/bin/sh
          exec python3 "$RUNNER_TEMP/release-plz-admission.py" cargo "$@"
          SIMIT_CARGO
          chmod 700 "$RUNNER_TEMP/release-plz-guard/cargo"
          cat > "$RUNNER_TEMP/release-plz-credentials" <<'SIMIT_CREDENTIALS'
          #!/bin/sh
          [ "$1" = get ] || exit 0
          protocol= host= path=
          while IFS= read -r line && [ -n "$line" ]; do
            case "$line" in
              protocol=*) protocol=${line#protocol=} ;;
              host=*) host=${line#host=} ;;
              path=*) path=${line#path=} ;;
            esac
          done
          [ "$protocol" = https ] && [ "$host" = github.com ] || exit 0
          [ "$path" = "$GITHUB_REPOSITORY" ] || [ "$path" = "$GITHUB_REPOSITORY.git" ] || exit 0
          printf 'username=x-access-token\npassword=%s\n' "$GIT_TOKEN"
          SIMIT_CREDENTIALS
          chmod 700 "$RUNNER_TEMP/release-plz-credentials"

"#,
    );
}

fn workflow(
    config: &ProjectConfig,
    runtime: Runtime,
    options: &CiOptions,
    packages: &[Package],
) -> String {
    let mut w = format!(
        "{}\n# release-plz computes updates and orchestrates publication and signed tags.\n# Build code runs only on credential-free workers; trusted jobs transport data.\n# Use an App/PAT: GITHUB_TOKEN cannot trigger PR checks or tag workflows.\n# Reruns skip published crates; recover missing tags explicitly (see CI adoption docs).\nname: Release-plz\n\non:\n  push:\n    branches: [\"**\"]\n\npermissions:\n  contents: read\n\nconcurrency:\n  group: release-plz-${{{{ github.repository }}}}-${{{{ github.run_id }}}}\n  cancel-in-progress: false\n\njobs:\n  select:\n    if: github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)\n    concurrency:\n      group: release-plz-admission-${{{{ github.repository }}}}-${{{{ github.ref }}}}\n      queue: max\n      cancel-in-progress: false\n    runs-on: ubuntu-24.04\n    timeout-minutes: 360\n    permissions:\n      actions: read\n      contents: read\n      pull-requests: read\n    outputs:\n      release_sha: ${{{{ steps.select.outputs.release_sha }}}}\n    steps:\n",
        ci::GENERATED_WORKFLOW_MARKER
    );
    checkout(&mut w);
    w.push_str("      - name: Require the preceding release-plz event to succeed\n        env:\n          GH_TOKEN: ${{ github.token }}\n        run: python3 .simit/release-plz.py prior-run\n\n");
    w.push_str(&format!("      - name: Resolve release-plz immutable revision\n        id: select\n        env:\n          GH_TOKEN: ${{{{ github.token }}}}\n          RELEASE_TRUST_ROOT: {:?}\n        run: python3 .simit/release-plz.py select\n\n  qualify:\n    needs: select\n    if: needs.select.outputs.release_sha != ''\n    runs-on: ubuntu-24.04\n    timeout-minutes: 120\n    permissions:\n      contents: read\n      pull-requests: read\n", config.release.signing.trust_root));
    ci::push_job_env(&mut w, Platform::Github, runtime, &options.extra_env, true);
    w.push_str("    steps:\n");
    checkout(&mut w);
    prepare(&mut w);
    w.push_str("      - name: Qualify exact release revision\n        env:\n          SIMIT_RELEASE_SHA: ${{ needs.select.outputs.release_sha }}\n        run: git checkout --detach \"$SIMIT_RELEASE_SHA\"\n\n");
    ci::release_plz_setup(&mut w, Runtime::Cargo, options);
    if runtime == Runtime::Nix {
        ci::release_plz_setup(&mut w, runtime, options);
    }
    ci::release_plz_checks(&mut w, runtime, packages, options);
    let selection = packages
        .iter()
        .filter(|package| package.is_publishable_to_crates_io())
        .map(|package| format!(" --package {}", ci::shell_word(&package.name)))
        .collect::<String>();
    let prefix = if runtime == Runtime::Nix {
        "nix develop -c "
    } else {
        ""
    };
    w.push_str(&format!("      - name: Verify package archives including unpublished workspace dependencies\n        run: |\n          set -euo pipefail\n          package_cargo=$(rustup which --toolchain stable cargo)\n          {prefix}\"$package_cargo\" package --locked --allow-dirty{selection} --target-dir \"$RUNNER_TEMP/release-plz-package\"\n\n      - name: Retain qualified crate archives\n        uses: {}\n        with:\n          name: release-plz-crates-${{{{ github.run_attempt }}}}\n          if-no-files-found: error\n          path: ${{{{ runner.temp }}}}/release-plz-package/package/*.crate\n\n", ci::github_action_ref("actions/upload-artifact", "v7.0.2")));
    w.push_str("  publish:\n    needs: [select, qualify]\n    runs-on: ubuntu-24.04\n    timeout-minutes: 60\n    permissions:\n      contents: read\n      pull-requests: read\n    steps:\n");
    checkout(&mut w);
    prepare(&mut w);
    ci::trusted_publisher::bootstrap(&mut w);
    w.push_str("      - name: Preserve qualified publisher outside project checkouts\n        run: cp .simit-publisher-engine/target/release/simit \"$RUNNER_TEMP/simit-release-plz-publisher\"\n\n");
    install(&mut w, Runtime::Cargo);
    w.push_str(&format!("      - name: Download same-run qualified archives\n        uses: {}\n        with:\n          name: release-plz-crates-${{{{ github.run_attempt }}}}\n          path: ${{{{ runner.temp }}}}/release-plz-archives\n\n", ci::github_action_ref("actions/download-artifact", "v8.0.2")));
    w.push_str(&format!("      - name: Recheck immutable release selection\n        working-directory: ${{{{ runner.temp }}}}/release-plz-event-source\n        env:\n          GH_TOKEN: ${{{{ github.token }}}}\n          SIMIT_EXPECTED_RELEASE_SHA: ${{{{ needs.select.outputs.release_sha }}}}\n          RELEASE_TRUST_ROOT: {:?}\n        run: python3 \"$RUNNER_TEMP/release-plz-admission.py\" select\n\n", config.release.signing.trust_root));
    w.push_str("      - name: Reconstruct qualified archives and reconcile all registry versions without credentials\n        env:\n          SIMIT_EXPECTED_RELEASE_SHA: ${{ needs.select.outputs.release_sha }}\n          SIMIT_RELEASE_PLZ_SOURCE: ${{ runner.temp }}/release-plz-event-source\n        run: |\n          \"$RUNNER_TEMP/simit-release-plz-publisher\" release publisher --publisher-stage prepare --repo \"$GITHUB_REPOSITORY\" --release-plz-archives \"$RUNNER_TEMP/release-plz-archives\" --release-plz-check-registry\n\n");
    signing(&mut w, config);
    let token = authentication(&mut w, config);
    w.push_str(&format!("      - name: Publish verified archives and signed tags\n        env:\n          GIT_TOKEN: {token}\n          CARGO_REGISTRY_TOKEN: ${{{{ secrets.{} }}}}\n          SIMIT_EXPECTED_RELEASE_SHA: ${{{{ needs.select.outputs.release_sha }}}}\n          SIMIT_RELEASE_PLZ_PUBLISHER: ${{{{ runner.temp }}}}/simit-release-plz-publisher\n        run: |\n          set -euo pipefail\n          test -n \"$GIT_TOKEN\" && test -n \"$CARGO_REGISTRY_TOKEN\" || {{ echo 'Missing release-plz automation/registry token' >&2; exit 1; }}\n          {}\n\n", config.ci.release_plz.registry_token_secret, invocation(Runtime::Cargo, "release")));
    cleanup(&mut w);
    w.push_str("  prepare-pr:\n    needs: [select, publish]\n    if: always() && needs.select.result == 'success' && (needs.publish.result == 'success' || (needs.select.outputs.release_sha == '' && needs.publish.result == 'skipped'))\n    runs-on: ubuntu-24.04\n    timeout-minutes: 120\n    permissions:\n      contents: read\n      pull-requests: read\n");
    ci::push_job_env(&mut w, Platform::Github, runtime, &options.extra_env, true);
    w.push_str("    steps:\n");
    checkout(&mut w);
    prepare(&mut w);
    ci::release_plz_setup(&mut w, runtime, options);
    ci::release_plz_update_setup(&mut w, options);
    install(&mut w, runtime);
    w.push_str(&format!("      - name: Compute release-plz versions and changelogs without release credentials\n        env:\n          GIT_TOKEN: ${{{{ github.token }}}}\n        run: |\n          set -euo pipefail\n          git switch --force-create \"$GITHUB_REF_NAME\" \"$GITHUB_SHA\"\n          {prefix}python3 \"$RUNNER_TEMP/release-plz-admission.py\" update --config \"$RUNNER_TEMP/release-plz.toml\" | tee \"$RUNNER_TEMP/release-plz-update.log\"\n          python3 \"$RUNNER_TEMP/release-plz-admission.py\" prepare-pr\n\n      - name: Retain release-plz update data\n        uses: {}\n        with:\n          name: release-plz-pr-${{{{ github.run_attempt }}}}\n          if-no-files-found: error\n          path: ${{{{ runner.temp }}}}/release-plz-pr/\n\n", ci::github_action_ref("actions/upload-artifact", "v7.0.2")));
    w.push_str("  release-pr:\n    needs: [select, prepare-pr]\n    if: always() && needs.select.result == 'success' && needs.prepare-pr.result == 'success'\n    runs-on: ubuntu-24.04\n    timeout-minutes: 30\n    concurrency:\n      group: release-plz-pr-${{ github.repository }}\n      cancel-in-progress: false\n    permissions:\n      contents: read\n      pull-requests: read\n    steps:\n");
    checkout(&mut w);
    prepare(&mut w);
    w.push_str(&format!("      - name: Download prepared release PR data\n        uses: {}\n        with:\n          name: release-plz-pr-${{{{ github.run_attempt }}}}\n          path: ${{{{ runner.temp }}}}/release-plz-pr\n\n      - name: Admit only release data into the event index\n        run: python3 \"$RUNNER_TEMP/release-plz-admission.py\" admit-pr\n\n", ci::github_action_ref("actions/download-artifact", "v8.0.2")));
    signing(&mut w, config);
    let token = authentication(&mut w, config);
    w.push_str(&format!("      - name: Deliver signed release-plz update without executing project code\n        env:\n          GIT_TOKEN: {token}\n          RELEASE_TRUST_ROOT: {:?}\n        run: |\n          set -euo pipefail\n          test -n \"$GIT_TOKEN\" || {{ echo 'Missing release-plz automation token' >&2; exit 1; }}\n          python3 \"$RUNNER_TEMP/release-plz-admission.py\" apply-pr\n\n", config.release.signing.trust_root));
    cleanup(&mut w);
    w.trim_end().to_owned() + "\n"
}

fn cleanup(w: &mut String) {
    w.push_str("      - name: Remove signing material\n        if: always()\n        run: |\n          git config --local --unset-all credential.helper || true\n          gpgconf --kill gpg-agent || true\n          rm -rf \"$RUNNER_TEMP/release-plz-gnupg\"\n\n");
}
