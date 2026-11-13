//! Explicit supply pin work items: rewrite one enrolled flake input, refreeze
//! its lock entry and hand both files to the normal candidate transaction.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use super::schema::PinItem;
use crate::project::{CiUpdatePlan, GeneratedFile};

/// Files a pin owns; no other scope may claim either, and a lock refresh may
/// touch nothing else.
pub(super) const PIN_FILES: [&str; 2] = ["flake.nix", "flake.lock"];

/// An item that cannot be applied at all. The repository transaction is
/// dropped before anything is written or committed and reported as skipped.
#[derive(Debug)]
pub(super) struct PinSkip(String);

impl PinSkip {
    pub(super) fn reason(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PinSkip {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "pin work item skipped: {}", self.0)
    }
}

impl std::error::Error for PinSkip {}

fn skip(reason: &'static str) -> anyhow::Error {
    anyhow::Error::new(PinSkip(reason.to_owned()))
}

/// Tests replace Nix with a deterministic lock rewriter; production resolves
/// the immutable store tool the updater unit runs.
#[cfg(test)]
pub(super) static NIX: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Pin one input on the disposable checkout and append both flake files to the
/// candidate plan. Nothing is written when the declaration is unrecognised.
pub(super) fn apply(
    checkout: &Path,
    state: &Path,
    plan: &mut CiUpdatePlan,
    item: &PinItem,
) -> Result<()> {
    for path in plan
        .files
        .iter()
        .map(|file| &file.relative_path)
        .chain(plan.patches.iter().map(|file| &file.relative_path))
        .chain(plan.obsolete.iter())
    {
        ensure!(
            !PIN_FILES
                .iter()
                .any(|owned| path.as_path() == Path::new(owned)),
            "pin work item conflicts with the enrolled flake scope at {}",
            path.display()
        );
    }
    regular_files(checkout)?;
    let flake = checkout.join("flake.nix");
    let source = match fs::read_to_string(&flake) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(skip("unrecognised-input-declaration"));
        }
        Err(error) => return Err(error.into()),
    };
    let rewritten = rewrite(&source, &item.input, &item.target).map_err(skip)?;
    let lock_path = checkout.join("flake.lock");
    let before: Value = crate::review::read_json(&lock_path)?;
    fs::write(&flake, &rewritten)?;
    let temporary = tempfile::Builder::new()
        .prefix("pin-nix-")
        .tempdir_in(state)?;
    if let Err(error) = lock(checkout, temporary.path()) {
        // Keep the disposable checkout consistent for diagnostics; the next
        // run replaces it before generating anything.
        fs::write(&flake, &source)?;
        return Err(error);
    }
    let after: Value = crate::review::read_json(&lock_path)?;
    verify_lock(&before, &after, &item.input, &item.target)?;
    verify_status(checkout)?;
    // README badges derive from the pre-pin checkout; the next run regenerates
    // them from the landed flake, like any other badge-input move.
    plan.patches
        .retain(|patch| patch.relative_path != Path::new("README.md"));
    for name in PIN_FILES {
        plan.push_patch(GeneratedFile {
            relative_path: name.into(),
            content: fs::read_to_string(checkout.join(name))?,
        });
    }
    Ok(())
}

/// A repository-controlled symlink would redirect the pin's writes outside the
/// disposable checkout, so both flake files must be regular files (or absent).
fn regular_files(checkout: &Path) -> Result<()> {
    for name in PIN_FILES {
        match fs::symlink_metadata(checkout.join(name)) {
            Ok(metadata) => ensure!(
                metadata.file_type().is_file(),
                "{name} is not a regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// `nix flake lock` resolves the rewritten URL. It gets a private home and
/// cache inside the state directory because the service home is read-only.
fn lock(checkout: &Path, temporary: &Path) -> Result<()> {
    let home = temporary.join("home");
    let cache = temporary.join("cache");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&cache)?;
    let mut command = std::process::Command::new(nix_tool()?);
    command
        .current_dir(checkout)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", &cache)
        .env("TMPDIR", temporary)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .args(["flake", "lock"]);
    // Only CA and Nix configuration the updater unit deliberately exports
    // survive the cleared environment.
    for key in [
        "SSL_CERT_FILE",
        "NIX_SSL_CERT_FILE",
        "NIX_CONFIG",
        "NIX_USER_CONF_FILES",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    super::engine::run_tool(command, 900).map(|_| ())
}

fn nix_tool() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = NIX
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    {
        return Ok(path);
    }
    super::sandbox::tool("nix")
}

/// The failing form is silent in Nix but would silently land the wrong input,
/// so a lock refresh may only rewrite what the pin asked for.
fn verify_status(checkout: &Path) -> Result<()> {
    let status = super::engine::git(checkout, &["status", "--porcelain", "-z"])?;
    for entry in status.split('\0').filter(|entry| !entry.is_empty()) {
        let path = entry
            .get(3..)
            .with_context(|| format!("malformed Git status entry {entry:?}"))?;
        ensure!(
            PIN_FILES.contains(&path),
            "nix flake lock modified unexpected path {path}"
        );
    }
    Ok(())
}

/// The pinned node must carry the target, and every other root input must keep
/// its exact locked node graph.
pub(super) fn verify_lock(before: &Value, after: &Value, input: &str, target: &str) -> Result<()> {
    let node = after["nodes"]["root"]["inputs"][input]
        .as_str()
        .with_context(|| format!("flake.lock root does not declare input {input}"))?;
    ensure!(
        after["nodes"][node]["locked"]["rev"].as_str() == Some(target),
        "flake.lock did not pin {input} to {target}"
    );
    let after_inputs = after["nodes"]["root"]["inputs"]
        .as_object()
        .context("flake.lock has no root inputs")?;
    for (name, value) in before["nodes"]["root"]["inputs"]
        .as_object()
        .context("previous flake.lock has no root inputs")?
    {
        if name == input {
            continue;
        }
        ensure!(
            after_inputs.get(name) == Some(value),
            "nix flake lock changed root input {name}"
        );
        // A root follows alias (an array path) is unchanged above; what it
        // reaches is covered by the input it follows, including the pin.
        let Some(node) = value.as_str() else {
            continue;
        };
        let mut previous = BTreeMap::new();
        let mut current = BTreeMap::new();
        locked_nodes(before, node, &mut previous)?;
        locked_nodes(after, node, &mut current)?;
        ensure!(
            previous == current,
            "nix flake lock changed locked nodes reachable from {name}"
        );
    }
    Ok(())
}

fn locked_nodes(lock: &Value, node: &str, nodes: &mut BTreeMap<String, Value>) -> Result<()> {
    if nodes.contains_key(node) {
        return Ok(());
    }
    let entry = lock["nodes"]
        .get(node)
        .with_context(|| format!("flake.lock lacks node {node}"))?;
    // The whole entry: locked metadata and labelled input edges alike.
    nodes.insert(node.to_owned(), entry.clone());
    if let Some(inputs) = entry.get("inputs").and_then(Value::as_object) {
        for value in inputs.values() {
            if let Some(target) = value.as_str() {
                locked_nodes(lock, target, nodes)?;
            }
        }
    }
    Ok(())
}

/// Rewrite the one URL literal that declares `input`. The three accepted
/// declaration forms are the ones enrolled consumers use; anything else is
/// refused rather than guessed at.
fn rewrite(source: &str, input: &str, target: &str) -> Result<String, &'static str> {
    let tokens = tokens(source);
    let mut containers: Vec<Option<String>> = Vec::new();
    let mut declarations = Vec::new();
    for index in 0..tokens.len() {
        match &tokens[index] {
            Token::Punct('{') => {
                let label = if index >= 2 && punct(tokens.get(index - 1), '=') {
                    ident(tokens.get(index - 2)).map(str::to_owned)
                } else {
                    None
                };
                containers.push(label);
            }
            Token::Punct('}') => {
                containers.pop();
            }
            _ => {}
        }
        let enclosing = containers.last().cloned().flatten();
        let at_top = containers.len() == 1 && enclosing.is_none();
        let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
        // `inputs.<input>.url = "…";`
        if ident(tokens.get(index)) == Some("inputs")
            && punct(tokens.get(index + 1), '.')
            && ident(tokens.get(index + 2)).is_some()
            && punct(tokens.get(index + 3), '.')
            && ident(tokens.get(index + 4)) == Some("url")
            && punct(tokens.get(index + 5), '=')
            && let Some((url, start, end)) = string(tokens.get(index + 6))
            && at_top
            && !punct(previous, '.')
        {
            declarations.push((
                ident(tokens.get(index + 2)).unwrap().to_owned(),
                url.to_owned(),
                start,
                end,
            ));
            continue;
        }
        // `<input>.url = "…";`
        if let Some(name) = ident(tokens.get(index))
            && name != "inputs"
            && punct(tokens.get(index + 1), '.')
            && ident(tokens.get(index + 2)) == Some("url")
            && punct(tokens.get(index + 3), '=')
            && let Some((url, start, end)) = string(tokens.get(index + 4))
            && (at_top || enclosing.as_deref() == Some("inputs"))
            && !punct(previous, '.')
        {
            declarations.push((name.to_owned(), url.to_owned(), start, end));
            continue;
        }
        // `<input> = { url = "…"; … };`
        if ident(tokens.get(index)) == Some("url")
            && !punct(previous, '.')
            && punct(tokens.get(index + 1), '=')
            && let Some((url, start, end)) = string(tokens.get(index + 2))
            && let Some(name) = enclosing
            && !matches!(
                name.as_str(),
                "inputs" | "outputs" | "description" | "nixConfig"
            )
            && (containers.len() == 2 && containers[0].is_none()
                || containers.len() == 3
                    && containers[0].is_none()
                    && containers[1].as_deref() == Some("inputs"))
        {
            declarations.push((name, url.to_owned(), start, end));
        }
    }
    let matching = declarations
        .into_iter()
        .filter(|(name, _, _, _)| name == input)
        .collect::<Vec<_>>();
    let [(_, url, start, end)] = matching.as_slice() else {
        return Err("unrecognised-input-declaration");
    };
    let rewritten = pin_url(url, target)?;
    Ok(format!(
        "{}{}{}",
        &source[..*start],
        rewritten,
        &source[*end..]
    ))
}

fn pin_url(url: &str, target: &str) -> Result<String, &'static str> {
    if let Some(rest) = url.strip_prefix("github:") {
        let parts = rest.split('/').collect::<Vec<_>>();
        return match parts.as_slice() {
            [owner, repository] if !owner.is_empty() && !repository.is_empty() => {
                Ok(format!("github:{owner}/{repository}/{target}"))
            }
            [owner, repository, revision]
                if !owner.is_empty() && !repository.is_empty() && is_revision(revision) =>
            {
                Ok(format!("github:{owner}/{repository}/{target}"))
            }
            [owner, repository, _] if !owner.is_empty() && !repository.is_empty() => {
                Err("branch-ref-github-url")
            }
            _ => Err("unrecognised-input-declaration"),
        };
    }
    if !url.starts_with("git+") {
        return Err("unrecognised-input-declaration");
    }
    let Some((base, query)) = url.split_once('?') else {
        return Ok(format!("{url}?rev={target}"));
    };
    let mut parameters = Vec::new();
    let mut revisions = 0;
    for parameter in query.split('&') {
        if let Some(revision) = parameter.strip_prefix("rev=") {
            if !is_revision(revision) {
                return Err("unrecognised-input-declaration");
            }
            revisions += 1;
            parameters.push(format!("rev={target}"));
        } else {
            parameters.push(parameter.to_owned());
        }
    }
    if revisions > 1 {
        return Err("unrecognised-input-declaration");
    }
    if revisions == 1 {
        Ok(format!("{base}?{}", parameters.join("&")))
    } else {
        Ok(format!("{url}&rev={target}"))
    }
}

fn is_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    String {
        value: String,
        start: usize,
        end: usize,
    },
    Punct(char),
}

/// Tokenize just enough Nix to find input URL assignments without matching
/// comments, plain strings or indented strings.
fn tokens(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
        } else if byte == b'#' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else if byte == b'"' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            tokens.push(Token::String {
                value: source[start..end].to_owned(),
                start,
                end,
            });
            index = (end + 1).min(bytes.len());
        } else if byte == b'\'' && bytes.get(index + 1) == Some(&b'\'') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'\'' && bytes[index + 1] == b'\'') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || matches!(bytes[index], b'_' | b'-' | b'\''))
            {
                index += 1;
            }
            tokens.push(Token::Ident(source[start..index].to_owned()));
        } else {
            tokens.push(Token::Punct(byte as char));
            index += 1;
        }
    }
    tokens
}

fn ident(token: Option<&Token>) -> Option<&str> {
    match token {
        Some(Token::Ident(name)) => Some(name),
        _ => None,
    }
}

fn punct(token: Option<&Token>, expected: char) -> bool {
    matches!(token, Some(Token::Punct(actual)) if *actual == expected)
}

fn string(token: Option<&Token>) -> Option<(&str, usize, usize)> {
    match token {
        Some(Token::String { value, start, end }) => Some((value, *start, *end)),
        _ => None,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use serde_json::json;

    const TARGET: &str = "1111111111111111111111111111111111111111";

    fn rewrites(source: &str, input: &str) -> Result<String, &'static str> {
        rewrite(source, input, TARGET)
    }

    #[test]
    fn rewrites_every_declared_url_form_and_keeps_unrelated_inputs() {
        let old = "a".repeat(40);
        let cases = [
            (
                format!(
                    "{{ inputs.foo.url = \"git+https://github.com/o/r.git?ref=trunk&rev={old}\"; }}"
                ),
                format!(
                    "{{ inputs.foo.url = \"git+https://github.com/o/r.git?ref=trunk&rev={TARGET}\"; }}"
                ),
            ),
            (
                "{ foo.url = \"github:o/r/".to_owned() + &old + "\"; }",
                format!("{{ foo.url = \"github:o/r/{TARGET}\"; }}"),
            ),
            (
                "{ foo = { url = \"git+https://github.com/o/r.git?ref=trunk\"; }; }".to_owned(),
                format!(
                    "{{ foo = {{ url = \"git+https://github.com/o/r.git?ref=trunk&rev={TARGET}\"; }}; }}"
                ),
            ),
            (
                "{ inputs.foo.url = \"github:o/r\"; }".to_owned(),
                format!("{{ inputs.foo.url = \"github:o/r/{TARGET}\"; }}"),
            ),
            (
                "{ inputs = { foo.url = \"git+ssh://git@example.com/o/r?shallow=1\"; }; }"
                    .to_owned(),
                format!(
                    "{{ inputs = {{ foo.url = \"git+ssh://git@example.com/o/r?shallow=1&rev={TARGET}\"; }}; }}"
                ),
            ),
            (
                format!(
                    "{{ foo = {{ url = \"git+https://github.com/o/r?rev={old}\"; }}; bar.url = \"github:o/b\"; }}"
                ),
                format!(
                    "{{ foo = {{ url = \"git+https://github.com/o/r?rev={TARGET}\"; }}; bar.url = \"github:o/b\"; }}"
                ),
            ),
        ];
        let cases = cases.into_iter().chain([(
            "{ inputs.foo = { url = \"github:o/r\"; inputs.bar.url = \"github:o/b\"; }; }"
                .to_owned(),
            format!(
                "{{ inputs.foo = {{ url = \"github:o/r/{TARGET}\"; inputs.bar.url = \"github:o/b\"; }}; }}"
            ),
        )]);
        for (source, expected) in cases {
            assert_eq!(rewrites(&source, "foo").unwrap(), expected, "{source}");
        }
        // Comments and indented strings never declare an input.
        let commented =
            "# foo.url = \"github:o/r\";\n{ bar = ''foo.url = \"github:o/r\";''; }".to_owned();
        assert_eq!(
            rewrites(&commented, "foo").unwrap_err(),
            "unrecognised-input-declaration"
        );
    }

    #[test]
    fn refuses_ambiguous_or_unsupported_urls() {
        let old = "a".repeat(40);
        for (source, reason) in [
            (
                "{ bar.url = \"github:o/r\"; }",
                "unrecognised-input-declaration",
            ),
            (
                "{ foo.url = \"github:o/r\"; inputs.foo.url = \"github:o/r\"; }",
                "unrecognised-input-declaration",
            ),
            (
                "{ inputs.foo.url = \"github:o/r/trunk\"; }",
                "branch-ref-github-url",
            ),
            (
                "{ foo = { url = \"https://example.com/flake.tar.gz\"; }; }",
                "unrecognised-input-declaration",
            ),
        ] {
            assert_eq!(rewrites(source, "foo").unwrap_err(), reason, "{source}");
        }
        let nested = format!("{{ foo = {{ inputs.bar.url = \"github:o/r/{old}\"; }}; }}");
        assert_eq!(
            rewrites(&nested, "bar").unwrap_err(),
            "unrecognised-input-declaration",
            "nested inputs are not root inputs"
        );
    }

    #[test]
    fn symlinked_flake_files_are_refused() {
        let checkout = tempfile::tempdir().unwrap();
        regular_files(checkout.path()).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), checkout.path().join("flake.nix")).unwrap();
        let error = regular_files(checkout.path()).unwrap_err().to_string();
        assert!(error.contains("flake.nix is not a regular file"), "{error}");
    }

    #[test]
    fn lock_verification_binds_the_target_and_preserves_other_graphs() {
        let old = "a".repeat(40);
        let lock = |harbor: &str, nested: &str| {
            json!({
                "nodes": {
                    "root": {"inputs": {"harbor": "harbor", "nixpkgs": "nixpkgs"}},
                    "harbor": {
                        "locked": {"rev": harbor, "type": "git"},
                        "inputs": {"nixpkgs": "nested"}
                    },
                    "nested": {"locked": {"rev": nested, "type": "github"}},
                    "nixpkgs": {
                        "locked": {"rev": old, "type": "github"},
                        "inputs": {"nested": "nested"}
                    }
                },
                "root": "root",
                "version": 7
            })
        };
        let before = lock(&old, &old);
        let after = lock(TARGET, &old);
        verify_lock(&before, &after, "harbor", TARGET).unwrap();
        verify_lock(&before, &lock(TARGET, &"b".repeat(40)), "harbor", TARGET).unwrap_err();
        verify_lock(&before, &lock(&old, &old), "harbor", TARGET).unwrap_err();
        let mut changed = lock(TARGET, &old);
        changed["nodes"]["nixpkgs"]["locked"]["rev"] = json!("b".repeat(40));
        verify_lock(&before, &changed, "harbor", TARGET).unwrap_err();
        // A root follows alias is compared by value; what it reaches moves
        // with the input it follows.
        let mut aliased = (before.clone(), lock(TARGET, &old));
        for lock in [&mut aliased.0, &mut aliased.1] {
            lock["nodes"]["root"]["inputs"]["crane"] = json!(["harbor", "nixpkgs"]);
        }
        verify_lock(&aliased.0, &aliased.1, "harbor", TARGET).unwrap();
        aliased.1["nodes"]["root"]["inputs"]["crane"] = json!(["nixpkgs"]);
        verify_lock(&aliased.0, &aliased.1, "harbor", TARGET).unwrap_err();
        // Swapping labelled edges keeps the node set and every locked payload.
        let mut swapped = (before.clone(), lock(TARGET, &old));
        for lock in [&mut swapped.0, &mut swapped.1] {
            lock["nodes"]["nixpkgs"]["inputs"] = json!({"x": "nested", "y": "other"});
            lock["nodes"]["other"] = json!({"locked": {"rev": "c".repeat(40)}});
        }
        verify_lock(&swapped.0, &swapped.1, "harbor", TARGET).unwrap();
        swapped.1["nodes"]["nixpkgs"]["inputs"] = json!({"x": "other", "y": "nested"});
        verify_lock(&swapped.0, &swapped.1, "harbor", TARGET).unwrap_err();
        let mut moved = lock(TARGET, &old);
        moved["nodes"]["root"]["inputs"]["nixpkgs"] = json!("elsewhere");
        verify_lock(&before, &moved, "harbor", TARGET).unwrap_err();
    }
}
