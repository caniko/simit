"""Keep native queue support strict without silencing other lint errors."""
import subprocess
import sys

for scope in ["workflow", "job"]:
    for queue, cancel, accepted in [
        ("max", "false", True),
        ("single", "true", True),
        ("max", "true", False),
        ("unlimited", "false", False),
        ("[max]", "false", False),
    ]:
        concurrency = f"concurrency:\n  group: release\n  queue: {queue}\n  cancel-in-progress: {cancel}\n"
        workflow = "on: push\n" + (concurrency if scope == "workflow" else "")
        workflow += "jobs:\n  check:\n    runs-on: ubuntu-latest\n"
        if scope == "job":
            workflow += "".join(f"    {line}\n" for line in concurrency.splitlines())
        workflow += "    steps:\n      - run: echo ok\n"
        result = subprocess.run([sys.argv[1], "-shellcheck=", "-pyflakes=", "-"],
                                input=workflow, text=True, capture_output=True)
        assert (result.returncode == 0) == accepted, (scope, queue, cancel, result.stdout, result.stderr)
        if accepted:
            bad = subprocess.run([sys.argv[1], "-shellcheck=", "-pyflakes=", "-"],
                                 input=workflow.replace("queue:", "queue-typo:"), text=True, capture_output=True)
            assert bad.returncode != 0, "unknown keys must remain errors"
