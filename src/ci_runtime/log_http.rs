//! Diagnostic native acquisition: explicit HTTPS redirects, isolated storage requests.
use super::{log_archive::Budget, log_custody::Identity};
use anyhow::{Context, Result, ensure};
use reqwest::{
    blocking::Client,
    header::{HeaderMap, LOCATION},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};
use url::Url;

fn client() -> Result<Client> {
    Ok(Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .https_only(true)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()?)
}

fn credential(budget: &Budget) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    super::process::query_secret(
        Command::new("gh").args(["auth", "token", "--hostname", "github.com"]),
        budget.remaining()?.min(std::time::Duration::from_secs(30)),
    )
}

fn api_request(
    client: &Client,
    endpoint: &str,
    budget: &mut Budget,
) -> Result<reqwest::blocking::Response> {
    let token = credential(budget)?;
    let text = std::str::from_utf8(&token)?.trim();
    ensure!(
        !text.is_empty() && !text.chars().any(char::is_control),
        "provider credential unavailable"
    );
    budget.request()?;
    client
        .get(endpoint)
        .bearer_auth(text)
        .header("User-Agent", "simit-native-log-observation-v1")
        .header("Accept", "application/vnd.github+json")
        .header("Accept-Encoding", "identity")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("Cache-Control", "no-cache")
        .timeout(budget.remaining()?)
        .send()
        .map_err(|_| anyhow::anyhow!("native API transport failed"))
}

pub(super) fn metadata(
    identity: &Identity,
    endpoint: &str,
    budget: &mut Budget,
) -> Result<Vec<u8>> {
    identity.validate()?;
    let url = format!(
        "https://api.github.com/repos/{}/{endpoint}",
        identity.repository
    );
    let mut response = api_request(&client()?, &url, budget)?;
    ensure!(
        response.status().as_u16() == 200,
        "provider metadata unavailable (HTTP {})",
        response.status().as_u16()
    );
    let mut raw = Vec::new();
    let mut buffer = [0; 65536];
    loop {
        budget.check()?;
        let n = response
            .read(&mut buffer)
            .map_err(|_| anyhow::anyhow!("provider metadata body interrupted"))?;
        if n == 0 {
            break;
        }
        ensure!(
            raw.len() + n <= 16 * 1024 * 1024,
            "provider metadata exceeds bound"
        );
        budget.charge_bytes(n as u64)?;
        raw.extend_from_slice(&buffer[..n]);
    }
    Ok(raw)
}

pub(super) struct CaptureOutcome {
    pub path: Option<PathBuf>,
    pub metadata: Value,
    pub failure: Option<Value>,
}

fn redirect(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.port().is_none_or(|p| p == 443),
        "unsupported native log redirect"
    );
    let host = url.host_str().context("redirect lacks hostname")?;
    ensure!(
        host == "results-receiver.actions.githubusercontent.com"
            || host.ends_with(".blob.core.windows.net"),
        "unenrolled native log storage host"
    );
    Ok(url)
}
fn headers(values: &HeaderMap) -> Value {
    let mut safe = serde_json::Map::new();
    for name in [
        "date",
        "content-type",
        "content-length",
        "x-github-request-id",
        "x-github-api-version-selected",
    ] {
        if let Some(value) = values.get(name).and_then(|v| v.to_str().ok()) {
            safe.insert(name.into(), json!(value));
        }
    }
    Value::Object(safe)
}
fn storage_request(
    client: &Client,
    target: Url,
    timeout: std::time::Duration,
) -> reqwest::blocking::RequestBuilder {
    client
        .get(target)
        .header("User-Agent", "simit-native-log-observation-v1")
        .header("Cache-Control", "no-cache")
        .header("Accept-Encoding", "identity")
        .timeout(timeout)
}
pub(super) fn capture(identity: &Identity, root: &Path, budget: &mut Budget) -> CaptureOutcome {
    let endpoint = format!(
        "https://api.github.com/repos/{}/actions/runs/{}/attempts/1/logs",
        identity.repository, identity.run
    );
    let mut outcome = CaptureOutcome {
        path: None,
        metadata: json!({"native_endpoint":endpoint,"authorization_forwarded_to_storage":false,
            "captured_at_claim":chrono::Utc::now().to_rfc3339(),
            "time_provenance":"current local acquisition clock and observed HTTP headers, not a trusted timestamp"}),
        failure: None,
    };
    let mut stage = "api_request";
    let result = (|| -> Result<()> {
        identity.validate()?;
        let client = client()?;
        let mut response = api_request(&client, &endpoint, budget)?;
        outcome.metadata["api_status"] = json!(response.status().as_u16());
        outcome.metadata["api_headers"] = headers(response.headers());
        let mut storage_hosts = vec![];
        stage = "storage_request";
        for _ in 0..2 {
            if !response.status().is_redirection() {
                break;
            }
            let target = redirect(
                response
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .context("native redirect missing")?,
            )?;
            storage_hosts.push(
                target
                    .host_str()
                    .context("storage host missing")?
                    .to_owned(),
            );
            outcome.metadata["storage_hosts"] = json!(storage_hosts);
            budget.request()?;
            // Authorization exists only on the exact API request above. No default
            // client headers, cookies or cloned API request reach a storage origin.
            response = storage_request(&client, target, budget.remaining()?)
                .send()
                .map_err(|_| anyhow::anyhow!("native storage transport failed"))?;
        }
        outcome.metadata["download_status"] = json!(response.status().as_u16());
        outcome.metadata["download_headers"] = headers(response.headers());
        ensure!(
            response.status().as_u16() == 200,
            "native archive unavailable (HTTP {})",
            response.status().as_u16()
        );
        if let Some(length) = response.content_length() {
            ensure!(
                length <= super::log_archive::Limits::default().bytes,
                "native archive exceeds download bound"
            );
        }
        ensure!(
            response
                .headers()
                .get("content-encoding")
                .is_none_or(|v| v == "identity"),
            "encoded native archive is unsupported"
        );
        let path = root.join(format!("{}-fresh-native.zip", identity.run));
        stage = "download_storage";
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        outcome.path = Some(path);
        stage = "download_body";
        let result = copy_body(&mut response, &mut file, budget, &mut outcome.metadata);
        let synced = file.sync_all();
        outcome.metadata["capture_file_synced"] = json!(synced.is_ok());
        result?;
        synced?;
        Ok(())
    })();
    if let Err(error) = result {
        outcome.failure = Some(super::log_reconcile::failure(&error, stage));
    }
    outcome
}

fn copy_body(
    input: &mut impl Read,
    output: &mut impl Write,
    budget: &mut Budget,
    metadata: &mut Value,
) -> Result<()> {
    let mut buffer = [0; 65536];
    let mut bytes = 0_u64;
    let mut hash = Sha256::new();
    let result = (|| {
        loop {
            budget.check()?;
            let n = input
                .read(&mut buffer)
                .map_err(|_| anyhow::anyhow!("native archive body interrupted"))?;
            if n == 0 {
                break;
            }
            budget.charge_bytes(n as u64)?;
            output.write_all(&buffer[..n])?;
            bytes += n as u64;
            hash.update(&buffer[..n]);
        }
        Ok(())
    })();
    metadata["bytes"] = json!(bytes);
    metadata["downloaded_sha256"] = json!(hex::encode(hash.finalize()));
    metadata["body_complete"] = json!(result.is_ok());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_redirects_cannot_change_scheme_port_or_receive_api_authority() {
        assert!(
            redirect("https://results-receiver.actions.githubusercontent.com/logs?sig=opaque")
                .is_ok()
        );
        for url in [
            "http://results-receiver.actions.githubusercontent.com/x",
            "https://api.github.com/x",
            "https://github.com@evil.invalid/x",
            "https://results-receiver.actions.githubusercontent.com:444/x",
            "https://results-receiver.actions.githubusercontent.com.evil.invalid/x",
            "https://account.blob.core.windows.net/x#fragment",
        ] {
            assert!(redirect(url).is_err(), "{url}");
        }
        let mut raw = HeaderMap::new();
        raw.insert(
            "location",
            "https://store.invalid/x?sig=secret".parse().unwrap(),
        );
        raw.insert("set-cookie", "secret".parse().unwrap());
        raw.insert("authorization", "secret".parse().unwrap());
        raw.insert("x-github-request-id", "public-request-id".parse().unwrap());
        let retained = headers(&raw).to_string();
        assert!(!retained.contains("secret"));
        assert!(retained.contains("public-request-id"));
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .build()
            .unwrap();
        let request = storage_request(
            &client,
            redirect("https://results-receiver.actions.githubusercontent.com/logs?sig=opaque")
                .unwrap(),
            std::time::Duration::from_secs(1),
        )
        .build()
        .unwrap();
        assert!(
            !request.headers().contains_key("authorization")
                && !request.headers().contains_key("cookie")
        );
    }
}
