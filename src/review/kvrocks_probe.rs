//! A bounded Unix-only PING establishes transport/sandbox admission, not compiler
//! cache acceptance. Exact wrapper/compiler write/hit receipts stay independent.
use anyhow::{Context, Result, ensure};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

pub fn ping(path: &Path) -> Result<()> {
    ping_until(path, Instant::now() + Duration::from_secs(2))
}

pub fn ping_until(path: &Path, deadline: Instant) -> Result<()> {
    let remaining = || -> Result<Duration> {
        let duration = deadline.saturating_duration_since(Instant::now());
        ensure!(!duration.is_zero(), "Kvrocks PING deadline exceeded");
        Ok(duration)
    };
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket
        .connect_timeout(&socket2::SockAddr::unix(path)?, remaining()?)
        .context("bounded Kvrocks Unix socket connection failed")?;
    let mut stream = UnixStream::from(std::os::fd::OwnedFd::from(socket));
    stream.set_write_timeout(Some(remaining()?))?;
    ensure!(
        stream.write(b"*1\r\n$4\r\nPING\r\n")? == 14,
        "incomplete bounded PING write"
    );
    let mut response = [0; 7];
    let mut received = 0;
    while received < response.len() {
        stream.set_read_timeout(Some(remaining()?))?;
        let count = stream.read(&mut response[received..])?;
        ensure!(count > 0, "Kvrocks closed its PING response");
        received += count;
        ensure!(
            response[..received] == b"+PONG\r\n"[..received],
            "Kvrocks did not return the exact RESP2 PONG"
        );
    }
    Ok(())
}

pub fn sandbox() -> Result<serde_json::Value> {
    let path = Path::new("/run/redis-sccache/redis.sock");
    ping(path)?;
    Ok(serde_json::json!({"socket": path, "resp2_ping": true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn unix_probe_requires_exact_pong_and_sends_no_shell_or_credentials() {
        for (response, accepted) in [
            (b"+PONG\r\n".as_slice(), true),
            (b"-NOAUTH\r\n", false),
            (b"+PONG\n", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("probe.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let server = std::thread::spawn(move || {
                let (mut peer, _) = listener.accept().unwrap();
                peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = [0; 14];
                peer.read_exact(&mut request).unwrap();
                assert_eq!(&request, b"*1\r\n$4\r\nPING\r\n");
                peer.write_all(response).unwrap();
            });
            assert_eq!(ping(&path).is_ok(), accepted);
            server.join().unwrap();
        }
    }

    #[test]
    fn full_unix_backlog_is_bounded_without_a_tcp_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("full.sock");
        let listener =
            socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
        listener
            .bind(&socket2::SockAddr::unix(&path).unwrap())
            .unwrap();
        listener.listen(0).unwrap();
        let _queued = UnixStream::connect(&path).unwrap();
        let start = std::time::Instant::now();
        assert!(ping(&path).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn trickled_pong_cannot_extend_the_whole_operation_deadline() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trickle.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            let mut request = [0; 14];
            peer.read_exact(&mut request).unwrap();
            for byte in b"+PONG\r\n" {
                std::thread::sleep(Duration::from_millis(40));
                if peer.write_all(&[*byte]).is_err() {
                    break;
                }
            }
        });
        let start = Instant::now();
        assert!(ping_until(&path, start + Duration::from_millis(80)).is_err());
        assert!(start.elapsed() < Duration::from_millis(200));
        server.join().unwrap();
    }
}
