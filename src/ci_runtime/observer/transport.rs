//! Fixed-origin authenticated GETs and credential-free bounded provider redirects.
use super::{Cancellation, Response, Transport};
use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    time::{Duration, Instant},
};

/// Native authenticated GitHub GETs; log redirects are HTTPS, bounded and credential-free.
pub struct GitHubTransport {
    client: reqwest::blocking::Client,
    token: String,
}
impl GitHubTransport {
    /// Create a fixed github.com transport. Credentials are never retained in evidence.
    pub fn new(token: String) -> Result<Self> {
        ensure!(!token.is_empty(), "missing GitHub token");
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent("simit-original-observer")
                .build()?,
            token,
        })
    }
}
fn approved_log_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|h| {
            h.ends_with(".blob.core.windows.net")
                || h == "results-receiver.actions.githubusercontent.com"
        })
}
impl Transport for GitHubTransport {
    fn get(
        &mut self,
        endpoint: &str,
        maximum: u64,
        timeout: Duration,
        cancellation: &Cancellation,
    ) -> Result<Response> {
        cancellation.check()?;
        ensure!(
            endpoint.starts_with("repos/")
                && !endpoint.contains(['\r', '\n', '#'])
                && !endpoint.contains(".."),
            "invalid GET endpoint"
        );
        let start = Instant::now();
        let mut url = url::Url::parse(&format!("https://api.github.com/{endpoint}"))?;
        for redirects in 0..=3 {
            let remaining = timeout
                .checked_sub(start.elapsed())
                .context("provider GET deadline exceeded")?;
            let mut request = self.client.get(url.clone()).timeout(remaining);
            if redirects == 0 {
                request = request
                    .bearer_auth(&self.token)
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28");
            }
            let mut response = match request.send() {
                Ok(response) => response,
                Err(error) => {
                    return Ok(Response {
                        status: 0,
                        body: Vec::new(),
                        retry_after: None,
                        transport_error: Some(
                            if error.is_timeout() {
                                "timeout"
                            } else if error.is_connect() {
                                "connect"
                            } else {
                                "request"
                            }
                            .into(),
                        ),
                    });
                }
            };
            if response.status().is_redirection() {
                ensure!(
                    (endpoint.ends_with("/logs") || endpoint.ends_with("/zip")) && redirects < 3,
                    "unsupported provider redirect"
                );
                url = url::Url::parse(
                    response
                        .headers()
                        .get("location")
                        .context("missing log redirect")?
                        .to_str()?,
                )?;
                ensure!(approved_log_url(&url), "unsupported log download origin");
                continue;
            }
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse().ok());
            let mut result = Response {
                status,
                body: Vec::new(),
                retry_after,
                transport_error: None,
            };
            let limit = if status == 200 {
                maximum
            } else {
                maximum.min(65536)
            };
            let mut buffer = [0; 65536];
            loop {
                if cancellation.check().is_err() {
                    result.transport_error = Some("cancelled".into());
                    break;
                }
                if start.elapsed() >= timeout {
                    result.transport_error = Some("timeout".into());
                    break;
                }
                match response.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let admitted =
                            (limit.saturating_sub(result.body.len() as u64) as usize).min(count);
                        result.body.extend_from_slice(&buffer[..admitted]);
                        if admitted < count {
                            result.transport_error = Some("byte-limit".into());
                            break;
                        }
                    }
                    Err(_) => {
                        result.transport_error = Some("body-read".into());
                        break;
                    }
                }
            }
            return Ok(result);
        }
        unreachable!("redirect loop always returns or rejects")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_downloads_cannot_forward_credentials_to_an_unapproved_origin() {
        assert!(approved_log_url(
            &url::Url::parse(
                "https://productionresultssa0.blob.core.windows.net/logs?signature=opaque"
            )
            .unwrap()
        ));
        for url in [
            "http://productionresultssa0.blob.core.windows.net/logs",
            "https://user:password@productionresultssa0.blob.core.windows.net/logs",
            "https://productionresultssa0.blob.core.windows.net:8443/logs",
            "https://blob.core.windows.net.attacker.invalid/logs",
            "https://api.github.com/logs",
        ] {
            assert!(!approved_log_url(&url::Url::parse(url).unwrap()), "{url}");
        }
    }
}
