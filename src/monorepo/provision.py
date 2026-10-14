"""Bound Nix provisioning before native startup, without another Nix call."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

started = Path(sys.argv[1])
remaining = max(0, int(sys.argv[2]) - time.time())
deadline = time.monotonic() + remaining
cancelled = 0


def cancel(signum, _frame):
    global cancelled
    cancelled = signum


for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
    signal.signal(signum, cancel)

if remaining == 0:
    print("Nix provisioning timed out before execution", file=sys.stderr)
    raise SystemExit(124)

with subprocess.Popen(sys.argv[3:], start_new_session=True) as process:
    status = 1
    try:
        while True:
            status = process.poll()
            if status is not None:
                break
            if cancelled or (not started.exists() and time.monotonic() >= deadline):
                print("Nix provisioning cancelled" if cancelled else "Nix provisioning timed out", file=sys.stderr)
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                # Native Simit owns its isolated gate and bounded diagnostics
                # after the marker. Give cancellation cleanup its reserved time.
                try:
                    process.wait(timeout=90 if started.exists() else 5)
                except subprocess.TimeoutExpired:
                    pass
                status = 128 + cancelled if cancelled else 124
                break
            time.sleep(0.05)
    finally:
        # Only unsuccessful paths own descendant cleanup. Successful setup
        # gates may retain services for a later component step.
        if process.poll() is None or cancelled or status != 0:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait()
raise SystemExit(status if status >= 0 else 128 - status)
