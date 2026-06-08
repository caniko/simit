use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::cargo;
use crate::cli::ReleaseCommand;

const MINISIGN_SECRET_KEY: &str = "MINISIGN_SECRET_KEY";
const MINISIGN_PASSWORD: &str = "MINISIGN_PASSWORD";
const DEFAULT_TOKEN_PATH: &str = ".local/share/berg-cli/codeberg.org/TOKEN";

pub fn init(command: ReleaseCommand) -> Result<()> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let repo = required_repo(&command)?;
    validate_repo(repo)?;
    let token = read_token(&command)?;
    let public_key_path = workspace_root.join(command.minisign_public_key.as_std_path());

    let minisign = if command.rotate_minisign {
        rotate_minisign_keypair(&public_key_path)?
    } else {
        import_minisign_keypair(&command, &public_key_path)?
    };
    verify_minisign_pair(&minisign.secret_key, &minisign.password, &public_key_path)?;

    let client = ForgejoClient::new(&command.secrets_api_base, repo, token)?;
    client.put_secret(MINISIGN_SECRET_KEY, &minisign.secret_key)?;
    client.put_secret(MINISIGN_PASSWORD, &minisign.password)?;

    let names = client.list_repo_secret_names()?;
    require_secret(&names, MINISIGN_SECRET_KEY)?;
    require_secret(&names, MINISIGN_PASSWORD)?;
    println!("uploaded release secrets for {repo}: {MINISIGN_SECRET_KEY}, {MINISIGN_PASSWORD}");
    Ok(())
}

pub fn check(command: ReleaseCommand) -> Result<()> {
    let repo = required_repo(&command)?;
    validate_repo(repo)?;
    let token = read_token(&command)?;
    let client = ForgejoClient::new(&command.secrets_api_base, repo, token)?;
    let mut names = client.list_repo_secret_names()?;
    names.extend(command.assumed_account_secrets.iter().cloned());

    require_secret(&names, MINISIGN_SECRET_KEY)?;
    require_secret(&names, MINISIGN_PASSWORD)?;
    require_secret(&names, "codeberg_token")?;
    println!("release secret names are configured for {repo}");
    Ok(())
}

pub fn inspect_minisign_input(command: ReleaseCommand) -> Result<()> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let public_key_path = workspace_root.join(command.minisign_public_key.as_std_path());
    if command.minisign_secret_key_file.is_none() || command.minisign_password_file.is_none() {
        bail!(
            "inspect-minisign-input requires --minisign-secret-key-file and --minisign-password-file"
        );
    }
    let secret_path = command
        .minisign_secret_key_file
        .as_ref()
        .expect("checked")
        .as_std_path();
    let password_path = command
        .minisign_password_file
        .as_ref()
        .expect("checked")
        .as_std_path();
    let raw_secret = fs::read_to_string(secret_path)
        .with_context(|| format!("reading minisign secret key from {}", secret_path.display()))?;
    let raw_password = fs::read_to_string(password_path)
        .with_context(|| format!("reading minisign password from {}", password_path.display()))?;
    let normalized_secret = normalize_secret_payload(&raw_secret, Some(MINISIGN_SECRET_KEY));
    let formatted_secret = ensure_minisign_secret_key_format(&normalized_secret);
    let normalized_password = normalize_secret_payload(&raw_password, Some(MINISIGN_PASSWORD));
    let public_key_present = public_key_path.exists();

    println!("minisign input inspection");
    println!("secret raw bytes: {}", raw_secret.len());
    println!("secret raw lines: {}", line_count(&raw_secret));
    println!("secret first line: {}", classify_first_line(&raw_secret));
    println!(
        "secret env wrapper: {}",
        env_wrapper_kind(&raw_secret, MINISIGN_SECRET_KEY)
    );
    println!(
        "secret quoted wrapper: {}",
        quote_wrapper_kind(raw_secret.trim())
    );
    println!("secret normalized bytes: {}", normalized_secret.len());
    println!(
        "secret normalized first line: {}",
        classify_first_line(&normalized_secret)
    );
    println!(
        "secret normalized base64 body: {}",
        looks_like_base64_key_body(normalized_secret.trim())
    );
    println!(
        "secret formatted first line: {}",
        classify_first_line(&formatted_secret)
    );
    println!(
        "secret formatting action: {}",
        if formatted_secret == normalized_secret {
            "unchanged"
        } else {
            "added-minisign-secret-header"
        }
    );
    println!("password raw bytes: {}", raw_password.len());
    println!("password raw lines: {}", line_count(&raw_password));
    println!(
        "password env wrapper: {}",
        env_wrapper_kind(&raw_password, MINISIGN_PASSWORD)
    );
    println!("password normalized bytes: {}", normalized_password.len());
    println!("public key present: {public_key_present}");

    if !public_key_present {
        println!("probe signing: skipped (public key missing)");
        return Ok(());
    }

    match probe_minisign_pair(&formatted_secret, &normalized_password, &public_key_path) {
        Ok(()) => println!("probe signing: ok"),
        Err(err) => println!("probe signing: failed: {err}"),
    }
    Ok(())
}

fn required_repo(command: &ReleaseCommand) -> Result<&str> {
    command
        .secrets_repo
        .as_deref()
        .ok_or_else(|| anyhow!("--repo OWNER/REPO is required with `simit release secrets`"))
}

fn validate_repo(repo: &str) -> Result<()> {
    let Some((owner, name)) = repo.split_once('/') else {
        bail!("--repo must be OWNER/REPO, got {repo}");
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        bail!("--repo must be OWNER/REPO, got {repo}");
    }
    Ok(())
}

fn read_token(command: &ReleaseCommand) -> Result<String> {
    if let Ok(token) = std::env::var("CODEBERG_TOKEN") {
        return clean_token(token);
    }
    if let Ok(token) = std::env::var("FORGEJO_TOKEN") {
        return clean_token(token);
    }
    if let Ok(token) = std::env::var("GITEA_TOKEN") {
        return clean_token(token);
    }
    let path = match &command.secrets_token_file {
        Some(path) => PathBuf::from(path.as_std_path()),
        None => default_token_path()?,
    };
    let token = fs::read_to_string(&path)
        .with_context(|| format!("reading Forgejo/Codeberg token from {}", path.display()))?;
    clean_token(token)
}

fn clean_token(token: String) -> Result<String> {
    let token = token.trim().to_owned();
    if token.is_empty() {
        bail!("Forgejo/Codeberg token is empty");
    }
    Ok(token)
}

fn default_token_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        anyhow!("HOME is unset; pass --token-file or set CODEBERG_TOKEN/FORGEJO_TOKEN")
    })?;
    Ok(PathBuf::from(home).join(DEFAULT_TOKEN_PATH))
}

struct MinisignPair {
    secret_key: String,
    password: String,
}

fn import_minisign_keypair(
    command: &ReleaseCommand,
    public_key_path: &Path,
) -> Result<MinisignPair> {
    if command.minisign_secret_key_file.is_none() && command.minisign_password_file.is_none() {
        bail!(
            "minisign import requires --minisign-secret-key-file and --minisign-password-file, or pass --rotate-minisign for a new keypair"
        );
    }
    if command.minisign_secret_key_file.is_none() || command.minisign_password_file.is_none() {
        bail!(
            "minisign import requires both --minisign-secret-key-file and --minisign-password-file"
        );
    }
    let secret_path = command
        .minisign_secret_key_file
        .as_ref()
        .expect("checked")
        .as_std_path();
    let password_path = command
        .minisign_password_file
        .as_ref()
        .expect("checked")
        .as_std_path();
    if !public_key_path.exists() {
        bail!(
            "minisign public key does not exist: {}",
            public_key_path.display()
        );
    }
    let secret_key = fs::read_to_string(secret_path)
        .with_context(|| format!("reading minisign secret key from {}", secret_path.display()))?;
    let password = fs::read_to_string(password_path)
        .with_context(|| format!("reading minisign password from {}", password_path.display()))?;
    let secret_key = ensure_minisign_secret_key_format(&normalize_secret_payload(
        &secret_key,
        Some(MINISIGN_SECRET_KEY),
    ));
    let password = normalize_secret_payload(&password, Some(MINISIGN_PASSWORD));
    if secret_key.trim().is_empty() {
        bail!("minisign secret key file is empty");
    }
    if password.is_empty() {
        bail!("minisign password file is empty");
    }
    Ok(MinisignPair {
        secret_key,
        password,
    })
}

fn normalize_secret_payload(input: &str, env_name: Option<&str>) -> String {
    let mut value = input.trim_end_matches(['\r', '\n']).trim_start().to_owned();
    if let Some(name) = env_name {
        if let Some(rest) = value
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
        {
            value = rest.trim_start().to_owned();
        } else {
            let prefix = format!("export {name}=");
            if let Some(rest) = value.strip_prefix(&prefix) {
                value = rest.trim_start().to_owned();
            }
        }
    }
    value = unwrap_shellish_quotes(&value);
    decode_common_escapes(&value)
}

fn ensure_minisign_secret_key_format(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with("untrusted comment:") {
        value.to_owned()
    } else if looks_like_base64_key_body(trimmed) {
        format!("untrusted comment: minisign encrypted secret key\n{trimmed}\n")
    } else {
        value.to_owned()
    }
}

fn looks_like_base64_key_body(value: &str) -> bool {
    !value.is_empty()
        && value.len() >= 80
        && !value.contains(char::is_whitespace)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

fn line_count(value: &str) -> usize {
    if value.is_empty() {
        0
    } else {
        value.lines().count()
    }
}

fn classify_first_line(value: &str) -> &'static str {
    let Some(first) = value.lines().next().map(str::trim) else {
        return "empty";
    };
    if first.starts_with("untrusted comment: minisign encrypted secret key") {
        "minisign-encrypted-secret-header"
    } else if first.starts_with("untrusted comment: minisign secret key") {
        "minisign-secret-header"
    } else if first.starts_with("untrusted comment:") {
        "other-minisign-comment"
    } else if first.starts_with("MINISIGN_SECRET_KEY=")
        || first.starts_with("export MINISIGN_SECRET_KEY=")
    {
        "minisign-secret-env-assignment"
    } else if first.starts_with("MINISIGN_PASSWORD=")
        || first.starts_with("export MINISIGN_PASSWORD=")
    {
        "minisign-password-env-assignment"
    } else if first.starts_with('/') || first.starts_with("./") || first.starts_with("../") {
        "path-like"
    } else if first.bytes().all(|b| b.is_ascii_hexdigit()) {
        "hex-like"
    } else if first
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        "base64-like"
    } else {
        "other"
    }
}

fn env_wrapper_kind(value: &str, name: &str) -> &'static str {
    let trimmed = value.trim_start();
    if trimmed.starts_with(&format!("{name}=")) {
        "name-equals"
    } else if trimmed.starts_with(&format!("export {name}=")) {
        "export-name-equals"
    } else {
        "none"
    }
}

fn quote_wrapper_kind(value: &str) -> &'static str {
    if value.len() < 2 {
        return "none";
    }
    match (value.as_bytes()[0], value.as_bytes()[value.len() - 1]) {
        (b'\'', b'\'') => "single",
        (b'"', b'"') => "double",
        _ => "none",
    }
}

fn unwrap_shellish_quotes(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

fn decode_common_escapes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('r') => {
                    chars.next();
                    out.push('\r');
                }
                Some('t') => {
                    chars.next();
                    out.push('\t');
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                }
                _ => out.push(ch),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn rotate_minisign_keypair(public_key_path: &Path) -> Result<MinisignPair> {
    let parent = public_key_path
        .parent()
        .ok_or_else(|| anyhow!("minisign public key path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let temp = PrivateTempDir::new("simit-minisign-rotate")?;
    let secret_path = temp.path().join("minisign.sec");
    let staged_public = temp.path().join("minisign.pub");
    let password = random_password()?;
    let mut child = Command::new("minisign")
        .arg("-G")
        .arg("-p")
        .arg(&staged_public)
        .arg("-s")
        .arg(&secret_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("running minisign -G")?;
    {
        let stdin = child.stdin.as_mut().expect("stdin piped");
        writeln!(stdin, "{password}")?;
        writeln!(stdin, "{password}")?;
    }
    let output = child
        .wait_with_output()
        .context("waiting for minisign -G")?;
    if !output.status.success() {
        bail!(
            "minisign -G failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::copy(&staged_public, public_key_path).with_context(|| {
        format!(
            "updating minisign public key {} from generated key",
            public_key_path.display()
        )
    })?;
    let secret_key = fs::read_to_string(&secret_path)
        .with_context(|| format!("reading generated {}", secret_path.display()))?;
    Ok(MinisignPair {
        secret_key,
        password,
    })
}

fn random_password() -> Result<String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(&mut bytes)
        .context("reading /dev/urandom")?;
    Ok(hex::encode(bytes))
}

fn verify_minisign_pair(secret_key: &str, password: &str, public_key_path: &Path) -> Result<()> {
    probe_minisign_pair(secret_key, password, public_key_path)
}

fn probe_minisign_pair(secret_key: &str, password: &str, public_key_path: &Path) -> Result<()> {
    let temp = PrivateTempDir::new("simit-minisign-verify")?;
    let secret_path = temp.path().join("minisign.sec");
    let message_path = temp.path().join("probe.txt");
    let signature_path = temp.path().join("probe.txt.minisig");
    write_private(&secret_path, secret_key.as_bytes())?;
    fs::write(&message_path, b"simit release secrets probe\n")?;

    let mut child = Command::new("minisign")
        .arg("-S")
        .arg("-s")
        .arg(&secret_path)
        .arg("-m")
        .arg(&message_path)
        .arg("-x")
        .arg(&signature_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("running minisign -S probe")?;
    {
        let stdin = child.stdin.as_mut().expect("stdin piped");
        writeln!(stdin, "{password}")?;
    }
    let output = child
        .wait_with_output()
        .context("waiting for minisign -S")?;
    if !output.status.success() {
        bail!(
            "minisign secret/password probe signing failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = Command::new("minisign")
        .arg("-V")
        .arg("-m")
        .arg(&message_path)
        .arg("-x")
        .arg(&signature_path)
        .arg("-p")
        .arg(public_key_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("running minisign -V probe")?;
    if !output.status.success() {
        bail!(
            "minisign secret/password do not verify with {}: {}",
            public_key_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

struct ForgejoClient {
    api_base: String,
    repo: String,
    token: String,
}

impl ForgejoClient {
    fn new(api_base: &str, repo: &str, token: String) -> Result<Self> {
        let api_base = api_base.trim_end_matches('/').to_owned();
        if api_base.is_empty() {
            bail!("--api-base must not be empty");
        }
        Ok(Self {
            api_base,
            repo: repo.to_owned(),
            token,
        })
    }

    fn put_secret(&self, name: &str, value: &str) -> Result<()> {
        let temp = PrivateTempDir::new("simit-forgejo-secret")?;
        let payload_path = temp.path().join("payload.json");
        let config_path = temp.path().join("curl.conf");
        let payload = serde_json::json!({ "data": value });
        write_private(&payload_path, serde_json::to_string(&payload)?.as_bytes())?;
        write_private(&config_path, self.curl_config().as_bytes())?;
        let url = format!(
            "{}/repos/{}/actions/secrets/{}",
            self.api_base, self.repo, name
        );
        let output = Command::new("curl")
            .arg("--config")
            .arg(&config_path)
            .arg("-fsS")
            .arg("-X")
            .arg("PUT")
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("--data-binary")
            .arg(format!("@{}", payload_path.display()))
            .arg(&url)
            .output()
            .with_context(|| format!("uploading secret {name} to {}", self.repo))?;
        if !output.status.success() {
            bail!(
                "uploading secret {name} to {} failed: {}",
                self.repo,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    fn list_repo_secret_names(&self) -> Result<BTreeSet<String>> {
        let temp = PrivateTempDir::new("simit-forgejo-secret-list")?;
        let config_path = temp.path().join("curl.conf");
        write_private(&config_path, self.curl_config().as_bytes())?;
        let url = format!("{}/repos/{}/actions/secrets", self.api_base, self.repo);
        let output = Command::new("curl")
            .arg("--config")
            .arg(&config_path)
            .arg("-fsS")
            .arg(&url)
            .output()
            .with_context(|| format!("listing release secrets for {}", self.repo))?;
        if !output.status.success() {
            bail!(
                "listing release secrets for {} failed: {}",
                self.repo,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        parse_secret_names(&output.stdout)
    }

    fn curl_config(&self) -> String {
        format!("header = \"Authorization: token {}\"\n", self.token)
    }
}

fn parse_secret_names(bytes: &[u8]) -> Result<BTreeSet<String>> {
    let value: Value = serde_json::from_slice(bytes).context("parsing Forgejo secret list JSON")?;
    let mut names = BTreeSet::new();
    collect_secret_names(&value, &mut names);
    Ok(names)
}

fn collect_secret_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_secret_names(item, names);
            }
        }
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                names.insert(name.to_owned());
            }
            for key in ["data", "secrets"] {
                if let Some(child) = map.get(key) {
                    collect_secret_names(child, names);
                }
            }
        }
        _ => {}
    }
}

fn require_secret(names: &BTreeSet<String>, name: &str) -> Result<()> {
    if names.contains(name) {
        Ok(())
    } else {
        bail!("release secret {name} is not configured")
    }
}

struct PrivateTempDir {
    path: PathBuf,
}

impl PrivateTempDir {
    fn new(prefix: &str) -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before UNIX_EPOCH")?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).with_context(|| format!("creating {}", path.display()))?;
        set_private_permissions(&path, true)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    set_private_permissions(path, false)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path, _directory: bool) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codeberg_secret_list_shapes() {
        let names = parse_secret_names(
            br#"{"data":[{"name":"MINISIGN_SECRET_KEY"},{"name":"MINISIGN_PASSWORD"}]}"#,
        )
        .unwrap();
        assert!(names.contains(MINISIGN_SECRET_KEY));
        assert!(names.contains(MINISIGN_PASSWORD));

        let names = parse_secret_names(br#"[{"name":"codeberg_token"}]"#).unwrap();
        assert!(names.contains("codeberg_token"));
    }

    #[test]
    fn rejects_invalid_repo_names() {
        assert!(validate_repo("owner/repo").is_ok());
        assert!(validate_repo("owner").is_err());
        assert!(validate_repo("owner/repo/extra").is_err());
    }

    #[test]
    fn normalizes_env_wrapped_secret_payloads() {
        assert_eq!(
            normalize_secret_payload(
                "MINISIGN_SECRET_KEY='untrusted comment: minisign encrypted secret key\\nabc\\n'",
                Some(MINISIGN_SECRET_KEY)
            ),
            "untrusted comment: minisign encrypted secret key\nabc\n"
        );
        assert_eq!(
            normalize_secret_payload(
                "export MINISIGN_PASSWORD=\"pass\\n\"",
                Some(MINISIGN_PASSWORD)
            ),
            "pass\n"
        );
        assert_eq!(
            normalize_secret_payload("raw secret\n", Some(MINISIGN_SECRET_KEY)),
            "raw secret"
        );
    }

    #[test]
    fn reconstructs_body_only_minisign_secret_key() {
        let body = "A".repeat(120);
        let normalized = ensure_minisign_secret_key_format(&body);
        assert!(normalized.starts_with("untrusted comment: minisign encrypted secret key\n"));
        assert!(normalized.ends_with('\n'));

        let full = "untrusted comment: minisign encrypted secret key\nabc\n";
        assert_eq!(ensure_minisign_secret_key_format(full), full);
    }

    #[test]
    fn classifies_secret_input_shapes() {
        assert_eq!(
            classify_first_line("untrusted comment: minisign encrypted secret key\nabc"),
            "minisign-encrypted-secret-header"
        );
        assert_eq!(
            classify_first_line("MINISIGN_SECRET_KEY=abc"),
            "minisign-secret-env-assignment"
        );
        assert_eq!(classify_first_line("/run/agenix/key"), "path-like");
        assert_eq!(
            classify_first_line(&format!("{}+/=", "Z".repeat(100))),
            "base64-like"
        );
        assert_eq!(
            env_wrapper_kind("export MINISIGN_PASSWORD=x", MINISIGN_PASSWORD),
            "export-name-equals"
        );
        assert_eq!(quote_wrapper_kind("'abc'"), "single");
    }
}
