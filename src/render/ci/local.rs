//! Local gate script derived from the rendered CI workflows, so a project whose
//! forge runs no CI can run the same checks before pushing. Project-owned
//! `[ci].local_gate_steps` follow the derived steps.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::GENERATED_WORKFLOW_MARKER;
use crate::config::CiConfig;
use crate::project::GeneratedFile;

pub const LOCAL_GATE_PATH: &str = "scripts/ci-local.sh";

/// Steps that only make sense on a hosted runner: runner variables, Actions
/// expressions, and edits to the developer's global Git configuration.
fn runner_only(command: &str) -> bool {
    command.contains("$GITHUB_")
        || command.contains("${{")
        || command.contains("git config --global")
}

fn workflow_label(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let stem = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))?;
    let label = stem.strip_prefix("ci")?;
    Some(label.strip_prefix('-').unwrap_or(label).to_owned())
}

/// Ordered `(label, step name, command)` from every `ci*.yaml` workflow. Job
/// `env` is deliberately ignored: hosted runners redirect `CARGO_HOME` and
/// caches into `/tmp`, which must not leak into a developer's machine.
fn workflow_steps(files: &[GeneratedFile]) -> Result<Vec<(String, String, String)>> {
    let mut steps = Vec::new();
    for file in files {
        if file.relative_path.starts_with(".crow") {
            crow_workflow_steps(file, &mut steps)?;
            continue;
        }
        let Some(label) = workflow_label(&file.relative_path) else {
            continue;
        };
        let workflow: serde_yaml::Value = serde_yaml::from_str(&file.content)?;
        let Some(jobs) = workflow.get("jobs").and_then(serde_yaml::Value::as_mapping) else {
            continue;
        };
        for job in jobs.values() {
            for step in job
                .get("steps")
                .and_then(serde_yaml::Value::as_sequence)
                .into_iter()
                .flatten()
            {
                let (Some(name), Some(run)) = (
                    step.get("name").and_then(serde_yaml::Value::as_str),
                    step.get("run").and_then(serde_yaml::Value::as_str),
                ) else {
                    continue;
                };
                let command = run.trim_end().to_owned();
                if !runner_only(&command) {
                    steps.push((label.clone(), name.to_owned(), command));
                }
            }
        }
    }
    Ok(steps)
}

fn crow_workflow_steps(
    file: &GeneratedFile,
    output: &mut Vec<(String, String, String)>,
) -> Result<()> {
    use serde_yaml::Value;

    let Some(stem) = file
        .relative_path
        .file_stem()
        .and_then(|stem| stem.to_str())
    else {
        return Ok(());
    };
    // Only replay CI lanes, never workflows that can publish or deploy.
    let label = match stem {
        "build" | "ci" => "",
        stem => match stem
            .strip_prefix("build-")
            .or_else(|| stem.strip_prefix("ci-"))
        {
            Some(label) => label,
            None => return Ok(()),
        },
    };
    let source = if file
        .relative_path
        .extension()
        .is_some_and(|ext| ext == "jsonnet")
    {
        // Simit's Jsonnet output is JSON with a generated marker comment.
        file.content.split_once('\n').map_or("", |(_, json)| json)
    } else {
        &file.content
    };
    let workflow: Value = serde_yaml::from_str(source)?;
    let manual_only = |value: &Value| {
        value
            .get("when")
            .and_then(Value::as_sequence)
            .is_some_and(|triggers| {
                !triggers.is_empty()
                    && triggers.iter().all(|trigger| {
                        let event = &trigger["event"];
                        event.as_str() == Some("manual")
                            || event.as_sequence().is_some_and(|events| {
                                !events.is_empty()
                                    && events.iter().all(|event| event.as_str() == Some("manual"))
                            })
                    })
            })
    };
    if manual_only(&workflow) {
        return Ok(());
    }
    for step in workflow
        .get("steps")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
    {
        let Some(name) = step.get("name").and_then(Value::as_str) else {
            continue;
        };
        if name == "nix-config" || manual_only(step) {
            continue;
        }
        let command = step
            .get("commands")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|command| {
                !matches!(
                    *command,
                    crate::render::crow::SCCACHE_PROBE | crate::render::crow::SCCACHE_STATS
                )
            })
            .map(|command| command.trim_end().replace("$$", "$"))
            .filter(|command| !runner_only(command))
            .collect::<Vec<_>>()
            .join("\n");
        if !command.is_empty() {
            output.push((label.to_owned(), name.to_owned(), command));
        }
    }
    Ok(())
}

fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

pub fn local_gate_script(files: &[GeneratedFile], ci: &CiConfig) -> Result<GeneratedFile> {
    let steps = workflow_steps(files)?;
    if steps.is_empty() && ci.local_gate_steps.is_empty() {
        bail!("[ci].local_gate needs Actions or Crow CI workflows to derive steps from");
    }
    let mut workflows_per_command: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, _, command) in &steps {
        *workflows_per_command.entry(command).or_default() += 1;
    }
    let mut script = format!(
        "#!/usr/bin/env bash\n{GENERATED_WORKFLOW_MARKER}\n\
# Runs the generated CI checks locally, in CI order. Each step runs like a\n\
# hosted CI step (bash -eo pipefail) without the runner's job env.\n\
#   bash {LOCAL_GATE_PATH} --list              list steps\n\
#   SIMIT_LOCAL_ONLY='<regex>' bash {LOCAL_GATE_PATH}  run matching steps only\n\
set -euo pipefail\n\
cd \"$(dirname \"$0\")/..\"\n\
list=0\n\
[ \"${{1:-}}\" = --list ] && list=1\n\
index=0\n\
step() {{\n\
  index=$((index + 1))\n\
  if [ \"$list\" = 1 ]; then printf '%3d  %s\\n' \"$index\" \"$1\"; return; fi\n\
  if [ -n \"${{SIMIT_LOCAL_ONLY:-}}\" ] && ! [[ $1 =~ $SIMIT_LOCAL_ONLY ]]; then return; fi\n\
  printf '\\n==> [%d] %s\\n' \"$index\" \"$1\"\n\
  bash -eo pipefail -c \"$2\" || {{ printf 'FAILED: %s\\n' \"$1\" >&2; exit 1; }}\n\
}}\n\n"
    );
    let mut seen = std::collections::BTreeSet::new();
    for (label, name, command) in &steps {
        if !seen.insert(command.as_str()) {
            continue;
        }
        // Shared steps (format, flake check, audit, deny) run once, unlabelled.
        let title = if workflows_per_command[command.as_str()] > 1 || label.is_empty() {
            name.clone()
        } else {
            format!("{label}: {name}")
        };
        writeln!(
            script,
            "step {} {}",
            single_quoted(&title),
            single_quoted(command)
        )
        .expect("write");
    }
    let build = &ci.nix_build;
    for installable in &ci.nix_builds {
        let mut command = String::from("nix build --no-link --no-update-lock-file");
        if let Some(jobs) = build.max_jobs {
            write!(command, " --max-jobs {jobs}").expect("write");
        }
        if let Some(cores) = build.cores {
            write!(command, " --cores {cores}").expect("write");
        }
        write!(
            command,
            " --print-build-logs {}",
            single_quoted(installable)
        )
        .expect("write");
        writeln!(
            script,
            "step {} {}",
            single_quoted(&format!("Nix build {installable}")),
            single_quoted(&command)
        )
        .expect("write");
    }
    for extra in &ci.local_gate_steps {
        writeln!(
            script,
            "step {} {}",
            single_quoted(&extra.name),
            single_quoted(&extra.run)
        )
        .expect("write");
    }
    Ok(GeneratedFile {
        // ponytail: written without the executable bit (run via `bash`), and
        // the registry's ci=drift scan only covers workflow dirs; disabling
        // `local_gate` retires the script through `init ci`.
        relative_path: PathBuf::from(LOCAL_GATE_PATH),
        content: script,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LocalGateStep;

    #[test]
    fn local_only_steps_do_not_need_derived_workflow_steps() {
        let mut ci = CiConfig::default();
        assert!(local_gate_script(&[], &ci).is_err());
        ci.local_gate_steps.push(LocalGateStep {
            name: "Browser suite".into(),
            run: "npm test".into(),
        });
        let script = local_gate_script(&[], &ci).unwrap();
        assert!(script.content.contains("step 'Browser suite' 'npm test'"));
    }
}
