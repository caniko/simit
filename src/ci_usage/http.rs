//! Bounded, credential-scoped JSON reads for CI usage providers.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 2048;
const MAX_ATTEMPTS: usize = 3;

/// One bearer credential per API origin. The token is never included in errors.
pub(super) struct JsonApi {
    client: reqwest::blocking::Client,
    base: url::Url,
    token: String,
    label: &'static str,
    accept: &'static str,
    github: bool,
}

impl JsonApi {
    pub(super) fn new(
        base: &str,
        token: String,
        label: &'static str,
        accept: &'static str,
        github: bool,
    ) -> Result<Self> {
        let trimmed = base.trim_end_matches('/');
        let base = url::Url::parse(&format!("{trimmed}/"))
            .with_context(|| format!("{label} API URL is invalid"))?;
        ensure!(
            base.username().is_empty() && base.password().is_none(),
            "{label} API URL must not embed credentials"
        );
        ensure!(
            base.scheme() == "https" || (base.scheme() == "http" && is_loopback(&base)),
            "{label} API URL must use HTTPS"
        );
        ensure!(!token.trim().is_empty(), "{label} token is empty");
        let token = token.trim().to_owned();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("simit-ci-usage")
            .build()?;
        Ok(Self {
            client,
            base,
            token,
            label,
            accept,
            github,
        })
    }

    pub(super) fn get(&self, path: &str) -> Result<Value> {
        let url = self.base.join(path)?;
        let mut last_error = None;
        for attempt in 0..MAX_ATTEMPTS {
            let mut request = self
                .client
                .get(url.clone())
                .header(reqwest::header::ACCEPT, self.accept)
                .bearer_auth(&self.token);
            if self.github {
                request = request.header("X-GitHub-Api-Version", "2022-11-28");
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error) if attempt + 1 < MAX_ATTEMPTS && transient_transport(&error) => {
                    last_error = Some(error);
                    backoff(attempt);
                    continue;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("{} transport failed for {path}", self.label));
                }
            };
            let status = response.status();
            let retry_after = header_seconds(&response, "retry-after");
            let remaining = response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if status.is_success() {
                let bytes = read_body(response, MAX_BODY_BYTES)?;
                return serde_json::from_slice(&bytes).with_context(|| {
                    format!("{} API returned invalid JSON for {path}", self.label)
                });
            }
            let body = read_body(response, MAX_ERROR_BODY_BYTES).unwrap_or_default();
            let snippet = snippet(&body);
            if rate_limited(status, remaining.as_deref(), retry_after.is_some()) {
                if let Some(seconds) = retry_after.filter(|seconds| *seconds <= 15)
                    && attempt + 1 < MAX_ATTEMPTS
                {
                    std::thread::sleep(Duration::from_secs(seconds));
                    continue;
                }
                let reset = reset
                    .as_deref()
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_else(|| "unknown".into());
                bail!(
                    "{} API rate limit exhausted (HTTP {status}, resets {reset}); retry later or narrow the window{}",
                    self.label,
                    suffix(&snippet)
                );
            }
            if transient_status(status) && attempt + 1 < MAX_ATTEMPTS {
                if let Some(seconds) = retry_after.filter(|seconds| *seconds <= 15) {
                    std::thread::sleep(Duration::from_secs(seconds));
                } else {
                    backoff(attempt);
                }
                continue;
            }
            let hint = match (self.label, status.as_u16()) {
                ("GitHub", 401) => {
                    "; check GITHUB_TOKEN/GH_TOKEN or `gh auth login`, and Actions read access"
                }
                ("GitHub", 404) => {
                    "; check the repository name and that the credential can read Actions"
                }
                ("Crow", 401 | 403) => "; check CROW_TOKEN and repository access",
                ("Crow", 404) => "; check CROW_SERVER, the repository name and token access",
                _ => "",
            };
            bail!(
                "{} API GET {path} returned HTTP {status}{hint}{}",
                self.label,
                suffix(&snippet)
            );
        }
        let error = last_error.expect("retry loop retains the last transport error");
        Err(error).with_context(|| format!("{} transport failed for {path}", self.label))
    }
}

fn is_loopback(url: &url::Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

fn transient_transport(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect() || error.is_request()
}

fn transient_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

fn rate_limited(
    status: reqwest::StatusCode,
    remaining: Option<&str>,
    has_retry_after: bool,
) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (status == reqwest::StatusCode::FORBIDDEN
            && (remaining == Some("0") || has_retry_after))
}

fn header_seconds(response: &reqwest::blocking::Response, name: &str) -> Option<u64> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
}

fn backoff(attempt: usize) {
    std::thread::sleep(Duration::from_millis(500 * (1 << attempt)));
}

fn read_body(response: reqwest::blocking::Response, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    response.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "API response exceeds the {limit} byte limit"
    );
    Ok(bytes)
}

fn snippet(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        format!("{short}...")
    } else {
        short
    }
}

fn suffix(snippet: &str) -> String {
    if snippet.is_empty() {
        String::new()
    } else {
        format!(": {snippet}")
    }
}
