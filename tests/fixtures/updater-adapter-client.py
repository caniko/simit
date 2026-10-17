"""Network-only client for the local test oracle; oracle_url is enrolled in its wrapper."""
import pathlib
import sys
import urllib.request

request = urllib.request.Request(
    oracle_url,
    data=pathlib.Path(sys.argv[-1]).read_bytes(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(request, timeout=10) as response:
    print(response.read().decode())
