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
            client: trusted_client()?
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent("simit-original-observer")
                .build()?,
            token,
        })
    }
}

fn trusted_client() -> Result<reqwest::ClientBuilder> {
    // Unlike getaddrinfo's blocking pool, lookup futures and their resolver
    // belong to this transfer's runtime and cannot hold up runtime shutdown.
    // https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html#method.dns_resolver
    let builder = reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(std::sync::Arc::new(super::dns::OwnedResolver::default()));
    // Native Windows/macOS TLS uses OS trust stores, not SSL_CERT_* paths.
    // OpenSSL platforms instead install an explicit protected bundle after
    // disabling all default roots (including environment-selected roots).
    if let Some(bundle) = option_env!("SIMIT_TRUSTED_CA_BUNDLE") {
        let path = std::path::Path::new(bundle);
        ensure!(path.is_absolute(), "packaged CA bundle must be absolute");
        if path.is_file() {
            return client_with_bundle(builder, path);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for bundle in ["/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/cert.pem"] {
            let path = std::path::Path::new(bundle);
            if super::super::trusted_platform::protected_file(path) {
                return client_with_bundle(builder, path);
            }
        }
        anyhow::bail!("trusted platform CA bundle unavailable for observer transport");
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    Ok(builder)
}

fn client_with_bundle(
    mut builder: reqwest::ClientBuilder,
    path: &std::path::Path,
) -> Result<reqwest::ClientBuilder> {
    ensure!(path.is_absolute(), "trusted CA bundle must be absolute");
    let path = std::fs::canonicalize(path)?;
    let pem = super::super::group::read_bytes(&path, 2 * 1024 * 1024)?;
    let roots = reqwest::Certificate::from_pem_bundle(&pem)?;
    ensure!(!roots.is_empty(), "trusted CA bundle is empty");
    builder = builder.tls_built_in_root_certs(false);
    for root in roots {
        builder = builder.add_root_certificate(root);
    }
    Ok(builder)
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
                        Ok(Err(error)) if error.is_request() && !error.is_builder() => "request",
                        _ => "permanent-request",
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
    fn stalled_async_dns_is_owned_across_reused_client_runtimes_and_interruptible() {
        use hickory_resolver::config::{NameServerConfig, ResolverConfig};
        use reqwest::dns::Resolve;
        use std::{
            net::UdpSocket,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
                mpsc,
            },
            thread,
        };
        struct Counted {
            resolver: super::super::dns::OwnedResolver,
            active: Arc<AtomicUsize>,
        }
        struct Active(Arc<AtomicUsize>);
        impl Drop for Active {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        impl Resolve for Counted {
            fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
                let future = self.resolver.resolve(name);
                let active = self.active.clone();
                Box::pin(async move {
                    active.fetch_add(1, Ordering::SeqCst);
                    let _active = Active(active);
                    future.await
                })
            }
        }
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let address = socket.local_addr().unwrap();
        let mut nameserver = NameServerConfig::udp(address.ip());
        nameserver.connections[0].port = address.port();
        let config = ResolverConfig::from_parts(None, Vec::new(), vec![nameserver]);
        let active = Arc::new(AtomicUsize::new(0));
        let resolver = Arc::new(Counted {
            resolver: super::super::dns::OwnedResolver {
                config: Some(config),
            },
            active: active.clone(),
        });
        let transport = GitHubTransport {
            client: trusted_client()
                .unwrap()
                .dns_resolver(resolver)
                .build()
                .unwrap(),
            token: "inert-test-token".into(),
        };
        let (seen, received) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut query = [0; 4096];
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                match released.try_recv() {
                    Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                match socket.recv_from(&mut query) {
                    Ok((length, _)) => {
                        let _ = seen.send(
                            query[..length]
                                .windows(6)
                                .any(|name| name.eq_ignore_ascii_case(b"second")),
                        );
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) => {}
                    Err(error) => panic!("DNS fixture: {error}"),
                }
            }
        });
        for (name, cancel) in [("first", true), ("second", false)] {
            let cancellation = Cancellation::default();
            let start = Instant::now();
            let response = thread::scope(|scope| {
                let worker = scope.spawn(|| {
                    transport.get_from_url(
                        "repos/owner/repo/actions/runs/1/logs",
                        url::Url::parse(&format!("http://{name}.example.com/logs")).unwrap(),
                        1024,
                        if cancel {
                            Duration::from_secs(30)
                        } else {
                            Duration::from_millis(300)
                        },
                        &cancellation,
                    )
                });
                let wait_until = Instant::now() + Duration::from_secs(3);
                let mut observed = false;
                while let Ok(second) =
                    received.recv_timeout(wait_until.saturating_duration_since(Instant::now()))
                {
                    if second == (name == "second") {
                        observed = true;
                        break;
                    }
                }
                if cancel || !observed {
                    cancellation.cancel();
                }
                let response = worker.join().unwrap().unwrap();
                assert!(
                    observed,
                    "DNS query not observed for {name}: status={}, error={:?}",
                    response.status, response.transport_error
                );
                response
            });
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "DNS runtime join exceeded bound"
            );
            assert_eq!(
                active.load(Ordering::SeqCst),
                0,
                "DNS future survived transfer shutdown"
            );
            assert_eq!(
                response.transport_error.as_deref(),
                Some(if cancel { "cancelled" } else { "timeout" })
            );
        }
        release.send(()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn connection_reset_before_headers_is_a_transient_request_failure() {
        use std::{io::Read, net::TcpListener, thread};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url =
            url::Url::parse(&format!("http://{}/logs", listener.local_addr().unwrap())).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            // Drop without headers, matching request/protocol failures rather
            // than a connection-establishment or successful-body failure.
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
        assert_eq!(response.status, 0);
        assert_eq!(response.transport_error.as_deref(), Some("request"));
    }

    #[test]
    fn job_proxy_and_ca_environment_cannot_change_observer_transport_trust() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            process::Command,
            thread,
        };
        const NAME: &str = "ci_runtime::observer::transport::tests::job_proxy_and_ca_environment_cannot_change_observer_transport_trust";
        if let Ok(mode) = std::env::var("SIMIT_TRANSPORT_TEST_CHILD") {
            let url = url::Url::parse(&std::env::var("SIMIT_TRANSPORT_TEST_URL").unwrap()).unwrap();
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
            if mode == "proxy" {
                assert_eq!(
                    response.status, 200,
                    "transport={:?}, body={:?}",
                    response.transport_error, response.body
                );
                assert_eq!(response.body, b"origin");
            } else {
                assert_ne!(response.status, 200, "job-selected CA was trusted");
            }
            return;
        }
        fn accept(listener: TcpListener) -> Option<std::net::TcpStream> {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        return Some(stream);
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    _ => return None,
                }
            }
        }
        fn read_request(stream: &mut impl Read) {
            let mut request = Vec::new();
            let mut chunk = [0; 4096];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).unwrap();
                assert!(
                    read > 0 && request.len() + read <= 65536,
                    "incomplete test request headers"
                );
                request.extend_from_slice(&chunk[..read]);
            }
        }
        fn child(mode: &str, url: &str) -> Command {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", NAME, "--nocapture"])
                .env("SIMIT_TRANSPORT_TEST_CHILD", mode)
                .env("SIMIT_TRANSPORT_TEST_URL", url);
            for name in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "NO_PROXY",
                "no_proxy",
            ] {
                command.env_remove(name);
            }
            command
        }
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/logs", origin.local_addr().unwrap());
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        let origin = thread::spawn(move || {
            if let Some(mut stream) = accept(origin) {
                read_request(&mut stream);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\norigin",
                );
            }
        });
        let proxy = thread::spawn(move || {
            if let Some(mut stream) = accept(proxy) {
                read_request(&mut stream);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nproxy",
                );
                return true;
            }
            false
        });
        let mut command = child("proxy", &url);
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(name, &proxy_url);
        }
        let output = command.output().unwrap();
        origin.join().unwrap();
        assert!(
            !proxy.join().unwrap(),
            "job-selected proxy received a request"
        );
        assert!(output.status.success(), "{output:?}");

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use openssl::{
                asn1::Asn1Time,
                bn::BigNum,
                hash::MessageDigest,
                pkey::PKey,
                rsa::Rsa,
                ssl::{SslAcceptor, SslMethod},
                x509::{
                    X509, X509NameBuilder,
                    extension::{BasicConstraints, SubjectAlternativeName},
                },
            };
            let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", "localhost").unwrap();
            let name = name.build();
            let mut certificate = X509::builder().unwrap();
            certificate.set_version(2).unwrap();
            certificate
                .set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
                .unwrap();
            certificate.set_subject_name(&name).unwrap();
            certificate.set_issuer_name(&name).unwrap();
            certificate.set_pubkey(&key).unwrap();
            certificate
                .set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            certificate
                .set_not_after(&Asn1Time::days_from_now(1).unwrap())
                .unwrap();
            certificate
                .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
                .unwrap();
            let san = SubjectAlternativeName::new()
                .dns("localhost")
                .build(&certificate.x509v3_context(None, None))
                .unwrap();
            certificate.append_extension(san).unwrap();
            certificate.sign(&key, MessageDigest::sha256()).unwrap();
            let certificate = certificate.build();
            let temp = tempfile::tempdir().unwrap();
            let ca = temp.path().join("job-ca.pem");
            std::fs::write(&ca, certificate.to_pem().unwrap()).unwrap();
            let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
            acceptor.set_private_key(&key).unwrap();
            acceptor.set_certificate(&certificate).unwrap();
            let acceptor = acceptor.build();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!(
                "https://localhost:{}/logs",
                listener.local_addr().unwrap().port()
            );
            let server = thread::spawn(move || {
                if let Some(stream) = accept(listener)
                    && let Ok(mut stream) = acceptor.accept(stream)
                {
                    read_request(&mut stream);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\norigin",
                    );
                }
            });
            let output = child("ca", &url)
                .env("SSL_CERT_FILE", &ca)
                .env("SSL_CERT_DIR", temp.path())
                .output()
                .unwrap();
            server.join().unwrap();
            assert!(output.status.success(), "{output:?}");
        }
    }

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
            stream.set_nonblocking(false).unwrap();
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
