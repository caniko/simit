//! Explicit supply pin work items: rewrite one enrolled flake input, refreeze
//! its lock entry and hand both files to the normal candidate transaction.
use anyhow::{Context, Result, ensure};
use rnix::ast::{self, Attr, Expr, HasEntry, InterpolPart};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use super::schema::{Generator, PinItem};
use crate::project::{CiUpdatePlan, GeneratedFile};

/// Files a pin owns; no other scope may claim either, and a lock refresh may
/// touch nothing else.
const PIN_FILES: [&str; 2] = ["flake.nix", "flake.lock"];

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

#[cfg(test)]
thread_local! {
    /// Each test can substitute a deterministic lock rewriter.
    pub(super) static NIX: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Identify the generator by source, not by the consumer's local input name.
pub(super) fn generator_input(checkout: &Path, generator: &Generator) -> Result<Option<String>> {
    let path = checkout.join("flake.nix");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let source = fs::read_to_string(path)?;
    let lock_path = checkout.join("flake.lock");
    let lock: Value = match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.is_file() => crate::review::read_json(&lock_path)?,
        Ok(_) => anyhow::bail!("flake.lock is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error.into()),
    };
    let matches =
        |coordinate: Option<String>| coordinate.as_ref() == Some(&generator.source_coordinate);
    let inputs = match root_inputs(&source) {
        Ok(inputs) => inputs,
        // An unclassifiable flake only matters when it pins the generator:
        // then skip this repository instead of failing the whole rollout.
        Err(reason) => {
            let pins_generator = lock_inputs(&lock)
                .into_iter()
                .flat_map(|inputs| inputs.keys())
                .any(|name| {
                    root_node(&lock, name)
                        .is_some_and(|node| matches(original_coordinate(&node["original"])))
                });
            return if pins_generator {
                Err(skip(reason))
            } else {
                Ok(None)
            };
        }
    };
    let mut chosen = None;
    let names: BTreeSet<_> = inputs
        .keys()
        .chain(
            lock_inputs(&lock)
                .into_iter()
                .flat_map(|inputs| inputs.keys()),
        )
        .collect();
    for name in names {
        let declared = inputs.get(name);
        let node = root_node(&lock, name);
        // A source-only input or subflake is not the enrolled generator, even
        // if it points at the same repository. Check the declaration as well
        // as the lock, which may be stale.
        let url = declared
            .and_then(|input| input.url.as_ref())
            .map(|(url, _)| url.as_str());
        if declared.is_some_and(|input| input.flake == Some(false))
            || url.is_some_and(has_subflake_dir)
            || node.is_some_and(|node| {
                node["flake"] == Value::Bool(false)
                    || node["original"].get("dir").is_some()
                    || node["locked"].get("dir").is_some()
            })
        {
            continue;
        }
        let source_matches = matches(url.and_then(url_coordinate));
        let lock_matches = matches(node.and_then(|node| original_coordinate(&node["original"])));
        if source_matches || lock_matches {
            ensure!(
                source_matches && lock_matches,
                "generator input {name} lacks a matching literal declaration and root lock entry"
            );
            ensure!(
                chosen.replace(name.clone()).is_none(),
                "multiple flake inputs refer to the enrolled generator coordinate"
            );
        }
    }
    Ok(chosen)
}

fn url_coordinate(url: &str) -> Option<String> {
    if let Some(repo) = url.strip_prefix("github:") {
        let mut parts = repo.split(['/', '?', '#']);
        Some(format!("github.com/{}/{}", parts.next()?, parts.next()?).to_ascii_lowercase())
    } else {
        crate::registry::coordinate_from_remote(url.strip_prefix("git+")?)
    }
}

fn original_coordinate(original: &Value) -> Option<String> {
    match original["type"].as_str()? {
        "github" => Some(
            format!(
                "{}/{}/{}",
                original
                    .get("host")
                    .map_or(Some("github.com"), Value::as_str)?,
                original["owner"].as_str()?,
                original["repo"].as_str()?
            )
            .to_ascii_lowercase(),
        ),
        "git" => crate::registry::coordinate_from_remote(original["url"].as_str()?),
        _ => None,
    }
}

fn has_subflake_dir(url: &str) -> bool {
    url.split_once('?')
        .is_some_and(|(_, query)| query.split(['&', '#']).any(|part| part.starts_with("dir=")))
}

fn lock_inputs(lock: &Value) -> Option<&serde_json::Map<String, Value>> {
    let root = lock["root"].as_str()?;
    lock["nodes"][root]["inputs"].as_object()
}

fn root_node<'a>(lock: &'a Value, input: &str) -> Option<&'a Value> {
    lock["nodes"].get(lock_inputs(lock)?.get(input)?.as_str()?)
}

/// Refresh the enrolled generator before the read-only renderer sees the flake.
/// Report whether the flake files need to be included in the candidate.
pub(super) fn align_generator(
    checkout: &Path,
    state: &Path,
    input: &str,
    target: &str,
) -> Result<bool> {
    let item = PinItem {
        coordinate: String::new(),
        relative_path: String::new(),
        input: input.into(),
        target: target.into(),
    };
    regular_files(checkout)?;
    let source = fs::read_to_string(checkout.join("flake.nix"))?;
    let rewritten = rewrite(&source, input, target).map_err(anyhow::Error::msg)?;
    let locked: Value = crate::review::read_json(&checkout.join("flake.lock"))?;
    if source != rewritten || verify_lock(&locked, &locked, input, target).is_err() {
        apply(checkout, state, &item).context("align generator flake input")?;
        let after: Value = crate::review::read_json(&checkout.join("flake.lock"))?;
        verify_generator_move(&locked, &after, input)?;
        return Ok(true);
    }
    Ok(false)
}

fn verify_generator_move(before: &Value, after: &Value, input: &str) -> Result<()> {
    let previous = root_node(before, input).context("missing previous generator lock entry")?;
    let next = root_node(after, input).context("missing new generator lock entry")?;
    ensure!(
        next["flake"] != Value::Bool(false)
            && next["original"].get("dir").is_none()
            && next["locked"].get("dir").is_none()
            && original_coordinate(&next["original"]) == original_coordinate(&previous["original"]),
        "locking changed the generator input source"
    );
    if previous["locked"]["rev"] != next["locked"]["rev"] {
        let timestamp = |node: &Value| node["locked"]["lastModified"].as_u64();
        // The consumer checkout has no generator Git history to prove ancestry.
        // A consumer pinned ahead of the fleet generator is skipped, not failed.
        if !timestamp(previous)
            .zip(timestamp(next))
            .is_some_and(|(old, new)| new >= old)
        {
            return Err(skip("generator-pin-ahead"));
        }
    }
    Ok(())
}

/// Carry the final flake and lock into the plan, without taking ownership from
/// an explicitly enrolled generation scope.
pub(super) fn append_patches(checkout: &Path, plan: &mut CiUpdatePlan) -> Result<()> {
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
    for name in PIN_FILES {
        plan.push_patch(GeneratedFile {
            relative_path: name.into(),
            content: fs::read_to_string(checkout.join(name))?,
        });
    }
    Ok(())
}

/// Pin one input on the disposable checkout. Nothing is written when the
/// declaration is unrecognised.
pub(super) fn apply(checkout: &Path, state: &Path, item: &PinItem) -> Result<()> {
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
    verify_status(checkout)
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
    if let Some(path) = NIX.with(|fixture| fixture.borrow().clone()) {
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
    let node = root_node(after, input)
        .with_context(|| format!("flake.lock root does not declare input {input}"))?;
    ensure!(
        node["locked"]["rev"].as_str() == Some(target),
        "flake.lock did not pin {input} to {target}"
    );
    let after_inputs = lock_inputs(after).context("flake.lock has no root inputs")?;
    let before_inputs = lock_inputs(before).context("previous flake.lock has no root inputs")?;
    let before_names: BTreeSet<_> = before_inputs.keys().filter(|name| *name != input).collect();
    let after_names: BTreeSet<_> = after_inputs.keys().filter(|name| *name != input).collect();
    ensure!(
        before_names == after_names,
        "nix flake lock changed non-target root inputs: added {:?}, removed {:?}",
        after_names.difference(&before_names),
        before_names.difference(&after_names)
    );
    for name in before_names {
        let value = &before_inputs[name];
        let current = &after_inputs[name];
        // Nix renumbers node keys (`treefmt-nix_3` -> `treefmt-nix_5`) when a
        // lock changes, so graphs are compared by content, not by key. A root
        // follows alias (an array path) must stay the same path; what it
        // reaches is covered by the input it follows, including the pin.
        let unchanged = match (value.as_str(), current.as_str()) {
            (Some(previous), Some(next)) => {
                resolved(before, previous, &mut BTreeMap::new(), 0)?
                    == resolved(after, next, &mut BTreeMap::new(), 0)?
            }
            _ => value == current,
        };
        ensure!(unchanged, "nix flake lock changed root input {name}");
    }
    Ok(())
}

/// A lock node with every child key replaced by the child's resolved content,
/// so two locks compare equal exactly when their graphs do, whatever Nix
/// named the nodes. Follows paths stay as written.
fn resolved(
    lock: &Value,
    node: &str,
    memo: &mut BTreeMap<String, Value>,
    depth: usize,
) -> Result<Value> {
    if let Some(value) = memo.get(node) {
        return Ok(value.clone());
    }
    ensure!(depth < 256, "flake.lock nests too deep at node {node}");
    let entry = lock["nodes"]
        .get(node)
        .with_context(|| format!("flake.lock lacks node {node}"))?;
    let mut value = entry.clone();
    if let Some(inputs) = entry.get("inputs").and_then(Value::as_object) {
        let mut children = serde_json::Map::new();
        for (label, child) in inputs {
            let child = match child.as_str() {
                Some(key) => resolved(lock, key, memo, depth + 1)?,
                None => child.clone(),
            };
            children.insert(label.clone(), child);
        }
        value["inputs"] = Value::Object(children);
    }
    memo.insert(node.to_owned(), value.clone());
    Ok(value)
}

const UNKNOWN: &str = "unrecognised-input-declaration";

#[derive(Default)]
struct Input {
    url: Option<(String, rnix::TextRange)>,
    flake: Option<bool>,
}

fn literal(value: &ast::Str) -> Option<String> {
    let [InterpolPart::Literal(text)] = <[_; 1]>::try_from(value.normalized_parts()).ok()? else {
        return None;
    };
    Some(text)
}

fn attr_name(attr: Attr) -> Option<String> {
    match attr {
        Attr::Ident(name) => Some(name.to_string()),
        Attr::Str(value) => literal(&value),
        Attr::Dynamic(_) => None,
    }
}

fn root_inputs(source: &str) -> Result<BTreeMap<String, Input>, &'static str> {
    let parsed = rnix::Root::parse(source);
    if !parsed.errors().is_empty() {
        return Err(UNKNOWN);
    }
    let Some(Expr::AttrSet(root)) = parsed.tree().expr() else {
        return Err(UNKNOWN);
    };
    let mut inputs = BTreeMap::new();
    read_inputs(root, &[], source, &mut inputs)?;
    Ok(inputs)
}

// Descend only through literal containers on `inputs.<name>.{url,flake}`.
// Never search expressions (notably `outputs`) or nested input overrides.
fn read_inputs(
    set: ast::AttrSet,
    prefix: &[String],
    source: &str,
    inputs: &mut BTreeMap<String, Input>,
) -> Result<(), &'static str> {
    if set.rec_token().is_some() {
        return Err(UNKNOWN);
    }
    for inherit in set.inherits() {
        for attr in inherit.attrs() {
            let name = attr_name(attr).ok_or(UNKNOWN)?;
            if match prefix.len() {
                0 => name == "inputs",
                1 => true,
                _ => matches!(name.as_str(), "url" | "flake"),
            } {
                return Err(UNKNOWN);
            }
        }
    }
    for binding in set.attrpath_values() {
        let mut path = prefix.to_vec();
        let attrpath = binding.attrpath().ok_or(UNKNOWN)?;
        let mut attrs = attrpath.attrs();
        while path.len() < 3
            && let Some(attr) = attrs.next()
        {
            path.push(attr_name(attr).ok_or(UNKNOWN)?);
            if path[0] != "inputs" {
                break;
            }
        }
        if path.first().is_none_or(|name| name != "inputs") {
            continue;
        }
        if path.len() < 3 {
            let Some(Expr::AttrSet(child)) = binding.value() else {
                return Err(UNKNOWN);
            };
            read_inputs(child, &path, source, inputs)?;
        } else {
            let input = inputs.entry(path[1].clone()).or_default();
            match (path[2].as_str(), binding.value()) {
                ("url", Some(Expr::Str(value)))
                    if input.url.is_none() && attrs.next().is_none() =>
                {
                    fn range(node: &impl ast::AstNode) -> rnix::TextRange {
                        node.syntax().text_range()
                    }
                    let range = range(&value);
                    let url = literal(&value).ok_or(UNKNOWN)?;
                    // Only plain double-quoted URLs; replacing an escaped or
                    // indented literal would require re-encoding its contents.
                    if source[usize::from(range.start())..usize::from(range.end())]
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        != Some(&url)
                    {
                        return Err(UNKNOWN);
                    }
                    input.url = Some((url, range));
                }
                ("flake", Some(Expr::Ident(value)))
                    if input.flake.is_none() && attrs.next().is_none() =>
                {
                    input.flake = Some(value.to_string().parse().map_err(|_| UNKNOWN)?);
                }
                ("url" | "flake", _) => return Err(UNKNOWN),
                _ => {}
            }
        }
    }
    Ok(())
}

fn rewrite(source: &str, input: &str, target: &str) -> Result<String, &'static str> {
    let inputs = root_inputs(source)?;
    let (url, range) = inputs
        .get(input)
        .and_then(|input| input.url.as_ref())
        .ok_or(UNKNOWN)?;
    let rewritten = pin_url(url, target)?;
    Ok(format!(
        "{}{}{}",
        &source[..usize::from(range.start()) + 1],
        rewritten,
        &source[usize::from(range.end()) - 1..]
    ))
}

fn pin_url(url: &str, target: &str) -> Result<String, &'static str> {
    if let Some(rest) = url.strip_prefix("github:") {
        if rest.contains(['?', '#']) {
            return Err(UNKNOWN);
        }
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use serde_json::json;

    const TARGET: &str = "1111111111111111111111111111111111111111";

    fn generator() -> Generator {
        Generator {
            executable: "/generator".into(),
            source_revision: TARGET.into(),
            source_coordinate: "github.com/caniko/simit".into(),
            forge: "github".into(),
            executable_sha256: None,
            required_checks: vec![],
        }
    }

    fn github_original() -> Value {
        json!({"type":"github", "owner":"caniko", "repo":"simit"})
    }

    fn write_lock(root: &Path, name: &str, node: Value) {
        fs::write(
            root.join("flake.lock"),
            json!({"root":"actual_root", "nodes": {
                "actual_root":{"inputs":{(name):"node"}}, "node":node
            }})
            .to_string(),
        )
        .unwrap();
    }

    fn rewrites(source: &str, input: &str) -> Result<String, &'static str> {
        rewrite(source, input, TARGET)
    }

    #[test]
    fn never_rewrites_outputs_let_bindings() {
        let unsupported = format!(
            r#"{{
                inputs.tool = {{ type = "github"; owner = "caniko"; repo = "simit"; rev = "{TARGET}"; }};
                outputs = {{ self, tool }}: let tool.url = "github:caniko/simit"; in {{}};
            }}"#
        );
        assert!(rewrites(&unsupported, "tool").is_err());
        let root = tempfile::tempdir().unwrap();
        let lock = json!({"original":github_original(),"locked":{"rev":TARGET}});
        write_lock(root.path(), "tool", lock.clone());
        fs::write(root.path().join("flake.nix"), &unsupported).unwrap();
        assert!(generator_input(root.path(), &generator()).is_err());
        let quoted = r#"{
            inputs."simit".url = "github:caniko/simit";
            outputs = { self, simit }: let simit = { url = "github:caniko/simit"; };
                inputs.simit.flake = false; in {};
        }"#;
        write_lock(root.path(), "simit", lock);
        for source in [quoted.to_owned(), quoted.replace("\"simit\"", "simit")] {
            fs::write(root.path().join("flake.nix"), source).unwrap();
            assert_eq!(
                generator_input(root.path(), &generator())
                    .unwrap()
                    .as_deref(),
                Some("simit")
            );
        }
        assert_eq!(
            rewrites(quoted, "simit").unwrap(),
            quoted.replacen(
                "github:caniko/simit",
                &format!("github:caniko/simit/{TARGET}"),
                1
            )
        );
    }

    #[test]
    fn refuses_nonliteral_pin_bindings() {
        for source in [
            r#"{ inputs.foo.url = "github:o/${name}"; }"#,
            r#"{ inputs.foo.url = "github:o/r" + suffix; }"#,
            r#"{ inputs.foo.url = "github:o/r"; inputs.foo.flake = isFlake; }"#,
            r#"{ inputs.foo.url = "github:o/r"; inputs.${name}.flake = false; }"#,
            r#"{ inputs.foo.url = "github:o/r"; inputs.foo = { inherit flake; }; }"#,
            r#"{ inputs.foo.url = "github:o/r"; outputs = ; }"#,
        ] {
            assert!(rewrites(source, "foo").is_err(), "{source}");
        }
    }

    #[test]
    fn refuses_github_queries_and_fragments() {
        for url in [
            "github:o/r?rev=main",
            "github:o/r#fragment",
            "github:o?h/r",
            "github:o/r/main#fragment",
        ] {
            assert!(pin_url(url, TARGET).is_err(), "{url}");
        }
    }

    #[test]
    fn generator_discovery_requires_a_matching_regular_root_lock() {
        let root = tempfile::tempdir().unwrap();
        let flake = root.path().join("flake.nix");
        let lock = root.path().join("flake.lock");
        let source = r#"{ inputs.simit.url = "github:caniko/simit"; }"#;
        fs::write(&flake, source).unwrap();
        assert!(generator_input(root.path(), &generator()).is_err());
        write_lock(
            root.path(),
            "simit",
            json!({"original":{"type":"github","owner":"other","repo":"simit"}}),
        );
        assert!(generator_input(root.path(), &generator()).is_err());
        write_lock(root.path(), "other", json!({"original":github_original()}));
        assert!(generator_input(root.path(), &generator()).is_err());
        write_lock(
            root.path(),
            "simit",
            json!({"original":github_original(), "flake":false}),
        );
        assert_eq!(generator_input(root.path(), &generator()).unwrap(), None);
        write_lock(root.path(), "simit", json!({"original":github_original()}));
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("lock");
        fs::rename(&lock, &target).unwrap();
        std::os::unix::fs::symlink(&target, &lock).unwrap();
        assert!(
            generator_input(root.path(), &generator())
                .unwrap_err()
                .to_string()
                .contains("flake.lock is not a regular file")
        );
    }

    #[test]
    fn ignores_generator_subflakes() {
        let root = tempfile::tempdir().unwrap();
        let flake = root.path().join("flake.nix");
        write_lock(root.path(), "simit", json!({"original":github_original()}));
        for url in [
            "github:caniko/simit?dir=child",
            "git+https://github.com/caniko/simit?ref=main&dir=child",
        ] {
            fs::write(&flake, format!(r#"{{ inputs.simit.url = "{url}"; }}"#)).unwrap();
            assert_eq!(generator_input(root.path(), &generator()).unwrap(), None);
        }
        fs::write(&flake, r#"{ inputs.simit.url = "github:caniko/simit"; }"#).unwrap();
        let mut original = github_original();
        original["dir"] = json!("child");
        write_lock(root.path(), "simit", json!({"original":original}));
        assert_eq!(generator_input(root.path(), &generator()).unwrap(), None);
    }

    #[test]
    fn unclassifiable_flakes_skip_only_when_they_pin_the_generator() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("flake.nix"),
            r#"rec { inputs.simit.url = "github:caniko/simit"; }"#,
        )
        .unwrap();
        assert_eq!(generator_input(root.path(), &generator()).unwrap(), None);
        write_lock(root.path(), "simit", json!({"original": github_original()}));
        let error = generator_input(root.path(), &generator()).unwrap_err();
        assert!(error.downcast_ref::<PinSkip>().is_some(), "{error:#}");
    }

    #[test]
    fn generator_moves_require_nondecreasing_lock_timestamps() {
        let lock = |rev: &str, timestamp: Value| {
            json!({"root":"root", "nodes": {
                "root":{"inputs":{"simit":"node"}}, "node":{"original":github_original(),"locked":{"rev":rev,"lastModified":timestamp}}
            }})
        };
        let old = lock(&"a".repeat(40), json!(20));
        for timestamp in [json!(19), Value::Null] {
            let error = verify_generator_move(&old, &lock(TARGET, timestamp), "simit").unwrap_err();
            assert_eq!(
                error.downcast_ref::<PinSkip>().map(PinSkip::reason),
                Some("generator-pin-ahead")
            );
        }
        for timestamp in [20, 21] {
            verify_generator_move(&old, &lock(TARGET, json!(timestamp)), "simit").unwrap();
        }
    }

    #[test]
    fn matches_generator_coordinate_not_input_name() {
        let root = tempfile::tempdir().unwrap();
        let generator = generator();
        let old = "a".repeat(40);
        let source = format!(
            r#"{{ inputs = {{
          simit.url = "github:someone-else/simit";
          tool.url = "git+https://github.com/Caniko/Simit.git?ref=trunk&rev={old}";
        }}; }}"#
        );
        fs::write(root.path().join("flake.nix"), &source).unwrap();
        write_lock(
            root.path(),
            "tool",
            json!({"original":{"type":"git","url":"https://github.com/Caniko/Simit.git"}}),
        );
        let input = generator_input(root.path(), &generator).unwrap().unwrap();
        assert_eq!(input, "tool");
        assert_eq!(
            rewrites(&source, &input).unwrap(),
            source.replace(&old, TARGET)
        );
    }

    #[test]
    fn ignores_source_only_generator_inputs() {
        let root = tempfile::tempdir().unwrap();
        let generator = generator();
        fs::write(
            root.path().join("flake.nix"),
            r#"{ inputs.templates = { url = "github:caniko/simit"; flake = false; }; }"#,
        )
        .unwrap();
        fs::write(
            root.path().join("flake.lock"),
            json!({"nodes": {"root": {"inputs": {"templates": "templates"}}, "templates": {"flake": false}}, "root": "root", "version": 7}).to_string(),
        )
        .unwrap();
        assert_eq!(generator_input(root.path(), &generator).unwrap(), None);
        // The declaration wins over a stale lock, and without any lock.
        fs::write(
            root.path().join("flake.lock"),
            json!({"nodes": {"root": {"inputs": {"templates": "templates"}}, "templates": {}}, "root": "root", "version": 7}).to_string(),
        )
        .unwrap();
        assert_eq!(generator_input(root.path(), &generator).unwrap(), None);
        fs::remove_file(root.path().join("flake.lock")).unwrap();
        assert_eq!(generator_input(root.path(), &generator).unwrap(), None);
        // An unrelated root input is not the generator.
        fs::write(
            root.path().join("flake.nix"),
            r#"{ inputs.x.url = "github:o/r"; }"#,
        )
        .unwrap();
        write_lock(
            root.path(),
            "x",
            json!({"original":{"type":"github","owner":"o","repo":"r"}}),
        );
        assert_eq!(generator_input(root.path(), &generator).unwrap(), None);
    }

    #[test]
    fn ignores_nested_generator_overrides() {
        let source = r#"{
            inputs.generator.url = "github:caniko/simit";
            inputs.tools.url = "github:example/tools";
            inputs.tools.inputs = { other-simit.url = "github:caniko/simit"; };
            inputs.tools.inputs.another-simit = { url = "github:caniko/simit"; };
        }"#;
        let matches = root_inputs(source)
            .unwrap()
            .into_iter()
            .filter_map(|(name, input)| (input.url?.0 == "github:caniko/simit").then_some(name))
            .collect::<Vec<_>>();
        assert_eq!(matches, ["generator"]);
        assert!(rewrites(source, "other-simit").is_err());
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
                "{ inputs = { foo.url = \"github:o/r/".to_owned() + &old + "\"; }; }",
                format!("{{ inputs = {{ foo.url = \"github:o/r/{TARGET}\"; }}; }}"),
            ),
            (
                "{ inputs = { foo = { url = \"git+https://github.com/o/r.git?ref=trunk\"; }; }; }"
                    .to_owned(),
                format!(
                    "{{ inputs = {{ foo = {{ url = \"git+https://github.com/o/r.git?ref=trunk&rev={TARGET}\"; }}; }}; }}"
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
                    "{{ inputs.foo = {{ url = \"git+https://github.com/o/r?rev={old}\"; flake = false; }}; inputs.bar.url = \"github:o/b\"; }}"
                ),
                format!(
                    "{{ inputs.foo = {{ url = \"git+https://github.com/o/r?rev={TARGET}\"; flake = false; }}; inputs.bar.url = \"github:o/b\"; }}"
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
                "{ inputs.bar.url = \"github:o/r\"; }",
                "unrecognised-input-declaration",
            ),
            (
                "{ inputs.foo.url = \"github:o/r\"; inputs.foo = { url = \"github:o/r\"; }; }",
                "unrecognised-input-declaration",
            ),
            (
                "{ inputs.foo.url = \"github:o/r/trunk\"; }",
                "branch-ref-github-url",
            ),
            (
                "{ inputs.foo = { url = \"https://example.com/flake.tar.gz\"; }; }",
                "unrecognised-input-declaration",
            ),
        ] {
            assert_eq!(rewrites(source, "foo").unwrap_err(), reason, "{source}");
        }
        let nested = format!("{{ inputs.foo.inputs.bar.url = \"github:o/r/{old}\"; }}");
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
    fn lock_verification_rejects_added_unrelated_root_input() {
        let before = json!({"root": "root", "nodes": {
            "root": {"inputs": {"simit": "simit"}},
            "simit": {"locked": {"rev": "a".repeat(40)}},
        }});
        let mut after = before.clone();
        after["nodes"]["simit"]["locked"]["rev"] = json!(TARGET);
        verify_lock(&before, &after, "simit", TARGET).unwrap();
        after["nodes"]["root"]["inputs"]["extra"] = json!("extra");
        after["nodes"]["extra"] = json!({"locked": {"type": "path", "path": "./extra"}});
        assert!(verify_lock(&before, &after, "simit", TARGET).is_err());
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
        // Nix renumbers node keys when a lock changes; identical content under
        // new keys, including a renamed root input node, is unchanged.
        let mut renumbered = lock(TARGET, &old);
        let nodes = renumbered["nodes"].as_object_mut().unwrap();
        let nested = nodes.remove("nested").unwrap();
        nodes.insert("nested_2".into(), nested);
        let nixpkgs = nodes.remove("nixpkgs").unwrap();
        nodes.insert("nixpkgs_3".into(), nixpkgs);
        renumbered["nodes"]["harbor"]["inputs"]["nixpkgs"] = json!("nested_2");
        renumbered["nodes"]["nixpkgs_3"]["inputs"]["nested"] = json!("nested_2");
        renumbered["nodes"]["root"]["inputs"]["nixpkgs"] = json!("nixpkgs_3");
        verify_lock(&before, &renumbered, "harbor", TARGET).unwrap();
        renumbered["nodes"]["nested_2"]["locked"]["rev"] = json!("b".repeat(40));
        verify_lock(&before, &renumbered, "harbor", TARGET).unwrap_err();
    }
}
