//! Fixed-origin authenticated GETs and credential-free bounded provider redirects.
use super::{Cancellation, Response, Transport};
use anyhow::{Context, Result, ensure};
use std::{
    future::Future,
    time::{Duration, Instant},
};

/// Native authenticated GitHub GETs; log redirects are HTTPS, bounded and credential-free.
pub struct GitHubTransport {
    client: reqwest::Client,
    token: String,
}
impl GitHubTransport {
    /// Create a fixed github.com transport. Credentials are never retained in evidence.
    pub fn new(token: String) -> Result<Self> {
        ensure!(!token.is_empty(), "missing GitHub token");
        Ok(Self {
            client: reqwest::Client::builder()
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
        let url = api_url(endpoint)?;
        self.get_from_url(endpoint, url, maximum, timeout, cancellation)
    }
}
fn api_url(endpoint: &str) -> Result<url::Url> {
    let path = endpoint.split('?').next().context("missing GET path")?;
    ensure!(
        endpoint.starts_with("repos/")
            && !endpoint.contains(['\r', '\n', '#', '\\'])
            && path.split('/').all(|segment| !segment.is_empty()
                && !matches!(segment, "." | "..")
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))),
        "invalid GET endpoint"
    );
    Ok(url::Url::parse(&format!(
        "https://api.github.com/{endpoint}"
    ))?)
}

// Keep the same future alive across polling slices. A stalled header/body read
// is dropped on cancellation/deadline; no detached blocking transfer survives.
async fn interruptible<T>(
    future: impl Future<Output = T>,
    deadline: Instant,
    cancellation: &Cancellation,
) -> std::result::Result<T, &'static str> {
    let mut future = std::pin::pin!(future);
    loop {
        if cancellation.check().is_err() {
            return Err("cancelled");
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("timeout")?;
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), &mut future).await {
            Ok(result) => return Ok(result),
            Err(_) => continue,
        }
    }
}

impl GitHubTransport {
    fn get_from_url(
        &self,
        endpoint: &str,
        url: url::Url,
        maximum: u64,
        timeout: Duration,
        cancellation: &Cancellation,
    ) -> Result<Response> {
        // Own the runtime on a scoped worker so synchronous consumers can call
        // this boundary from a Tokio task. Joining owns all transfer work;
        // neither a nested runtime nor a detached request survives the call.
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    runtime.block_on(self.transfer(endpoint, url, maximum, timeout, cancellation))
                })
                .join()
                .map_err(|_| anyhow::anyhow!("provider transfer worker panicked"))?
        })
    }
    async fn transfer(
        &self,
        endpoint: &str,
        mut url: url::Url,
        maximum: u64,
        timeout: Duration,
        cancellation: &Cancellation,
    ) -> Result<Response> {
        let start = Instant::now();
        let deadline = start
            .checked_add(timeout)
            .context("provider GET deadline overflow")?;
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
            let mut response = match interruptible(request.send(), deadline, cancellation).await {
                Ok(Ok(response)) => response,
                failure => {
                    let error = match failure {
                        Err(reason) => reason,
                        Ok(Err(error)) if error.is_timeout() => "timeout",
                        Ok(Err(error)) if error.is_connect() => "connect",
                        _ => "request",
                    };
                    return Ok(Response {
                        status: 0,
                        body: Vec::new(),
                        retry_after: None,
                        transport_error: Some(error.into()),
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
            let mut result = Response {
                status,
                body: Vec::new(),
                retry_after: None,
                transport_error: None,
            };
            let limit = if status == 200 {
                maximum
            } else {
                maximum.min(65536)
            };
            loop {
                match interruptible(response.chunk(), deadline, cancellation).await {
                    Ok(Ok(None)) => break,
                    Ok(Ok(Some(chunk))) => {
                        let admitted = (limit.saturating_sub(result.body.len() as u64) as usize)
                            .min(chunk.len());
                        result.body.extend_from_slice(&chunk[..admitted]);
                        if admitted < chunk.len() {
                            result.transport_error = Some("byte-limit".into());
                            break;
                        }
                    }
                    failure => {
                        result.transport_error = Some(
                            match failure {
                                Err(reason) => reason,
                                Ok(Err(error)) if error.is_timeout() => "timeout",
                                _ => "body-read",
                            }
                            .into(),
                        );
                        break;
                    }
                }
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            result.retry_after = retry_delay(status, response.headers(), now);
            if status == 403
                && result.retry_after.is_none()
                && serde_json::from_slice::<serde_json::Value>(&result.body)
                    .ok()
                    .and_then(|body| {
                        body["message"].as_str().map(|message| {
                            message
                                .to_ascii_lowercase()
                                .contains("secondary rate limit")
                        })
                    })
                    == Some(true)
            {
                result.retry_after = Some(60);
            }
            return Ok(result);
        }
        unreachable!("redirect loop always returns or rejects")
    }
}

fn retry_delay(status: u16, headers: &reqwest::header::HeaderMap, now: u64) -> Option<u64> {
    if let Some(wait) = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        return Some(wait.max(1));
    }
    if matches!(status, 403 | 429)
        && headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            == Some("0")
    {
        return Some(
            headers
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map_or(60, |reset| reset.saturating_sub(now).saturating_add(1)),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_reset_and_secondary_limits_supply_provider_bounded_waits() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
            time::SystemTime,
        };
        for (status, remaining, reset, retry_after, message, expected) in [
            (403, Some("0"), true, None, "quota", Some(120)),
            // No provider window: observer orchestration owns minute-scale
            // exponential fallback and retains the selected delay.
            (429, None, false, None, "rate limit", None),
            (
                403,
                None,
                false,
                None,
                "You have exceeded a secondary rate limit",
                Some(60),
            ),
            (403, Some("0"), true, Some("7"), "quota", Some(7)),
            (403, None, false, None, "Forbidden", None),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = url::Url::parse(&format!("http://{}/logs", listener.local_addr().unwrap()))
                .unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                let body = serde_json::json!({"message":message}).to_string();
                let mut headers = format!(
                    "HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                if let Some(remaining) = remaining {
                    headers.push_str(&format!("x-ratelimit-remaining: {remaining}\r\n"));
                }
                if reset {
                    headers.push_str(&format!(
                        "x-ratelimit-reset: {}\r\n",
                        SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            + 120
                    ));
                }
                if let Some(wait) = retry_after {
                    headers.push_str(&format!("Retry-After: {wait}\r\n"));
                }
                stream
                    .write_all(format!("{headers}\r\n{body}").as_bytes())
                    .unwrap();
            });
            let response = GitHubTransport::new("inert-test-token".into())
                .unwrap()
                .get_from_url(
                    "repos/owner/repo/actions/runs/1/logs",
                    url,
                    1024,
                    Duration::from_secs(3),
                    &Cancellation::default(),
                )
                .unwrap();
            server.join().unwrap();
            if expected == Some(120) {
                assert!(
                    response
                        .retry_after
                        .is_some_and(|wait| (119..=121).contains(&wait)),
                    "reset-only primary limit lost its wait: {:?}",
                    response.retry_after
                );
            } else {
                assert_eq!(
                    response.retry_after, expected,
                    "status={status}, message={message}"
                );
            }
        }
    }

    #[test]
    fn synchronous_transfer_is_usable_from_a_tokio_task() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url =
            url::Url::parse(&format!("http://{}/logs", listener.local_addr().unwrap())).unwrap();
        let server = thread::spawn(move || {
            let start = Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(3) =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("test server accept: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nevidence",
                )
                .unwrap();
        });
        let transport = GitHubTransport::new("inert-test-token".into()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let response = runtime.block_on(async {
            transport
                .get_from_url(
                    "repos/owner/repo/actions/runs/1/logs",
                    url,
                    1024,
                    Duration::from_secs(3),
                    &Cancellation::default(),
                )
                .unwrap()
        });
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"evidence");
        assert!(response.transport_error.is_none());
        server.join().unwrap();
        // Creation and destruction must also work inside the caller's runtime.
        runtime.block_on(async {
            drop(GitHubTransport::new("inert-test-token".into()).unwrap());
        });
    }
    #[test]
    fn period_pairs_in_repository_names_are_not_path_traversal() {
        assert_eq!(
            api_url("repos/owner/foo..bar/actions/runs?per_page=100")
                .unwrap()
                .path(),
            "/repos/owner/foo..bar/actions/runs"
        );
        for endpoint in [
            "repos/owner/../actions/runs",
            "repos/owner/./actions/runs",
            "repos/owner/%2e%2e/actions/runs",
            "repos/owner/repo\\../actions/runs",
            "repos/owner/repo/actions/runs#fragment",
        ] {
            assert!(api_url(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn stalled_response_headers_and_bodies_are_interruptible_and_keep_received_bytes() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::mpsc,
            thread,
        };
        for send_headers in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = url::Url::parse(&format!("http://{}/logs", listener.local_addr().unwrap()))
                .unwrap();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut buffer = [0; 1024];
                assert!(stream.read(&mut buffer).unwrap() > 0);
                if send_headers {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial").unwrap();
                }
                ready_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            });
            let cancellation = Cancellation::default();
            let signal = cancellation.clone();
            let canceller = thread::spawn(move || {
                ready_rx.recv().unwrap();
                thread::sleep(Duration::from_millis(100));
                signal.cancel();
            });
            let transport = GitHubTransport::new("inert-test-token".into()).unwrap();
            let start = Instant::now();
            let result = transport
                .get_from_url(
                    "repos/owner/repo/actions/runs/1/logs",
                    url,
                    1024,
                    Duration::from_secs(3),
                    &cancellation,
                )
                .unwrap();
            let elapsed = start.elapsed();
            release_tx.send(()).unwrap();
            server.join().unwrap();
            canceller.join().unwrap();
            assert!(
                elapsed < Duration::from_secs(1),
                "cancellation waited {elapsed:?}"
            );
            assert_eq!(result.transport_error.as_deref(), Some("cancelled"));
            if send_headers {
                assert_eq!(result.body, b"partial");
            }
        }
    }
    #[test]
    fn streamed_bodies_keep_the_aggregate_deadline_and_byte_ceiling() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        for maximum in [4, 1024] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = url::Url::parse(&format!("http://{}/logs", listener.local_addr().unwrap()))
                .unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut buffer = [0; 1024];
                assert!(stream.read(&mut buffer).unwrap() > 0);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial").unwrap();
                for _ in 0..8 {
                    thread::sleep(Duration::from_millis(75));
                    if stream.write_all(b"more").is_err() {
                        break;
                    }
                }
            });
            let transport = GitHubTransport::new("inert-test-token".into()).unwrap();
            let start = Instant::now();
            let result = transport
                .get_from_url(
                    "repos/owner/repo/actions/runs/1/logs",
                    url,
                    maximum,
                    Duration::from_millis(250),
                    &Cancellation::default(),
                )
                .unwrap();
            let elapsed = start.elapsed();
            server.join().unwrap();
            assert!(
                elapsed < Duration::from_secs(1),
                "transfer waited {elapsed:?}"
            );
            if maximum == 4 {
                assert_eq!(result.body, b"part");
                assert_eq!(result.transport_error.as_deref(), Some("byte-limit"));
            } else {
                assert!(result.body.starts_with(b"partial"));
                assert_eq!(result.transport_error.as_deref(), Some("timeout"));
            }
        }
    }

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
