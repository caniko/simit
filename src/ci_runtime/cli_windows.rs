//! CLI-owned Windows console cancellation. Library observers install no handlers.
use super::super::cancellation::Cancellation;
use anyhow::{Result, ensure};
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};

static TOKEN: OnceLock<Cancellation> = OnceLock::new();
static FINISHED: AtomicBool = AtomicBool::new(false);

pub(super) fn register(token: Cancellation) -> Result<()> {
    ensure!(
        TOKEN.set(token).is_ok(),
        "Windows CLI cancellation already registered"
    );
    // SAFETY: the callback has the required system ABI, static lifetime and no
    // borrowed state. It only updates owned atomics and performs bounded waits.
    let registered = unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
    if registered == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) fn finished() {
    FINISHED.store(true, Ordering::SeqCst);
}

unsafe extern "system" fn handler(event: u32) -> i32 {
    if !matches!(
        event,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        return 0;
    }
    let Some(token) = TOKEN.get() else {
        return 0;
    };
    token.cancel();
    if matches!(
        event,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    ) {
        // Windows terminates the process when these callbacks return, even on
        // TRUE. Give the command its cleanup opportunity within the normal OS
        // five-second close budget. Ctrl+C/Break have no such OS deadline.
        let deadline = Instant::now() + Duration::from_secs(4);
        while !FINISHED.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_runtime::observer::{Identity, Response, Spec, Transport, observe};

    struct NoProvider;
    impl Transport for NoProvider {
        fn get(&mut self, _: &str, _: u64, _: Duration, _: &Cancellation) -> Result<Response> {
            panic!("cancelled Windows capture must not contact a provider")
        }
    }
    #[test]
    fn registered_console_callback_requests_cancellation_and_waits_for_close_cleanup() {
        let token = Cancellation::default();
        register(token.clone()).unwrap();
        for event in [
            CTRL_C_EVENT,
            CTRL_BREAK_EVENT,
            CTRL_CLOSE_EVENT,
            CTRL_LOGOFF_EVENT,
            CTRL_SHUTDOWN_EVENT,
        ] {
            token.0.store(false, Ordering::SeqCst);
            FINISHED.store(false, Ordering::SeqCst);
            let cleanup = std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                finished();
            });
            // SAFETY: invokes the registered static callback with documented
            // console event values; it has no pointer parameters or borrowed data.
            assert_eq!(unsafe { handler(event) }, 1);
            assert!(token.check().is_err());
            if event == CTRL_C_EVENT {
                let spec = Spec {
                    schema: "simit.original-observer-spec.v1".into(),
                    repository: "owner/repo".into(),
                    workflows: [(".github/workflows/ci.yaml".into(), vec!["test".into()])].into(),
                    timeout_seconds: 120,
                    poll_seconds: 60,
                };
                let identity = Identity {repository_id:1,head:"a".repeat(40),event:"push".into(),run_id:99,run_attempt:1,workflow_ref:"owner/repo/.github/workflows/custody.yaml@refs/heads/main".into(),workflow_sha:"a".repeat(40),event_json:serde_json::json!({"after":"a".repeat(40),"ref":"refs/heads/main","deleted":false,"repository":{"id":1,"full_name":"owner/repo"}}).to_string()};
                let root = tempfile::tempdir().unwrap();
                let packet = root.path().join("packet");
                assert!(observe(&spec, &identity, &packet, &mut NoProvider, &token).is_err());
                let receipt: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(packet.join("receipt.json")).unwrap())
                        .unwrap();
                assert_eq!(receipt["capture_complete"], false);
                assert_eq!(receipt["qualified"], false);
                assert!(receipt["error"].is_string());
            }
            if event == CTRL_CLOSE_EVENT {
                assert!(FINISHED.load(Ordering::SeqCst));
            }
            cleanup.join().unwrap();
        }
        token.0.store(false, Ordering::SeqCst);
        // SAFETY: an unknown scalar control value must be passed to other handlers.
        assert_eq!(unsafe { handler(123) }, 0);
        assert!(token.check().is_ok());
    }
}
