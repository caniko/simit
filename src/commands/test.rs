use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};
use tempfile::TempDir;

use crate::cli::TestCommand;

#[derive(Debug)]
pub struct TestExit(i32);

impl TestExit {
    pub fn code(&self) -> i32 {
        self.0
    }
}

impl fmt::Display for TestExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "test command exited with status {}", self.0)
    }
}

impl std::error::Error for TestExit {}

pub fn run(options: TestCommand) -> Result<()> {
    let Some((program, args)) = options.command.split_first() else {
        bail!("a test command is required after --");
    };
    let mut command = Command::new(program);
    command.args(args);
    let fixture = if options.git_fixtures {
        let fixture = FixtureEnvironment::new()?;
        fixture.configure(&mut command);
        eprintln!(
            "simit: Git fixture policy scoped to {}",
            fixture.root.display()
        );
        Some(fixture)
    } else {
        None
    };
    let status = command.status().context("running test command")?;
    // Cleanup happens before main propagates the child exit code.
    drop(fixture);
    if status.success() {
        Ok(())
    } else {
        Err(TestExit(exit_code(status)).into())
    }
}

fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

struct FixtureEnvironment {
    _directory: TempDir,
    root: PathBuf,
    config: PathBuf,
}

impl FixtureEnvironment {
    fn new() -> Result<Self> {
        if !cfg!(unix) {
            bail!("Git fixture isolation currently requires Unix");
        }
        // Ask Git for its config paths so XDG/HOME and explicit overrides retain
        // their original ordering. Includes stay relative to their source file.
        let output = Command::new("git")
            .args(["var", "GIT_CONFIG_GLOBAL"])
            .output()
            .context("discovering global Git configuration")?;
        if !output.status.success() {
            bail!(
                "could not discover global Git configuration; Git must support `git var GIT_CONFIG_GLOBAL`"
            );
        }
        let paths =
            String::from_utf8(output.stdout).context("Git configuration paths must be UTF-8")?;
        let directory = tempfile::Builder::new()
            .prefix("st-")
            .tempdir()
            .context("creating Git fixture temp root")?;
        let root = directory
            .path()
            .canonicalize()
            .context("resolving Git fixture temp root")?;
        let config = root.join("gitconfig");
        let mut contents = String::new();
        for path in paths.lines().filter(|line| !line.is_empty()) {
            let path = Path::new(path);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                env::current_dir()?.join(path)
            };
            // Git permits missing default global config files.
            if absolute.try_exists()? {
                contents.push_str(&format!(
                    "[include]\n\tpath = {}\n",
                    quote(path_text(&absolute)?)
                ));
            }
        }
        let fixture_config = root.join("fixture.gitconfig");
        let hooks = root.join("empty-hooks");
        fs::create_dir(&hooks)?;
        fs::write(
            &fixture_config,
            format!(
                "[core]\n\thooksPath = {}\n[commit]\n\tgpgSign = false\n[tag]\n\tgpgSign = false\n",
                quote(path_text(&hooks)?)
            ),
        )?;
        // A trailing slash makes Git match descendants. Escape wildmatch syntax
        // in the actual path so a temp parent like /tmp/test[1] cannot widen it.
        let pattern = format!("gitdir:{}/", escape_pattern(path_text(&root)?));
        contents.push_str(&format!(
            "[includeIf {}]\n\tpath = {}\n",
            quote(&pattern),
            quote(path_text(&fixture_config)?)
        ));
        fs::write(&config, contents)?;
        Ok(Self {
            _directory: directory,
            root,
            config,
        })
    }

    fn configure(&self, command: &mut Command) {
        command.env("GIT_CONFIG_GLOBAL", &self.config);
        for key in ["TMPDIR", "TMP", "TEMP"] {
            command.env(key, &self.root);
        }
    }
}

fn path_text(path: &Path) -> Result<&str> {
    let text = path.to_str().context("Git fixture paths must be UTF-8")?;
    if text.contains(['\n', '\r']) {
        bail!("Git fixture paths cannot contain newlines");
    }
    Ok(text)
}

fn quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\t', "\\t")
    )
}

fn escape_pattern(path: &str) -> String {
    let mut result = String::new();
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']') {
            result.push('\\');
        }
        result.push(c);
    }
    result
}
