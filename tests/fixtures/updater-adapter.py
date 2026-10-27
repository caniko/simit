#!/usr/bin/env python3
"""Local-only updater protocol oracle; never contacts a hosted forge."""
import json
import hashlib
import os
import pathlib
import subprocess
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


def respond(request, control_path, remote):
    control = json.loads(pathlib.Path(control_path).read_text())

    def git(*args):
        return subprocess.check_output(["git", "--git-dir", remote, *args], text=True).strip()

    # Pull requests outlive control rewrites, like forge state does.
    pulls_path = pathlib.Path(control_path).with_name("pulls.json")
    pulls = json.loads(pulls_path.read_text()) if pulls_path.exists() else []

    def merge(pull, style):
        # The forge, not the operator, authors merge, squash and rebase commits.
        identity = {f"GIT_{role}_{key}": value for role in ("AUTHOR", "COMMITTER")
                    for key, value in (("NAME", "Fixture forge"), ("EMAIL", "forge@example.invalid"))}
        base = git("rev-parse", f"refs/heads/{pull['base']}")
        if style == "ff":
            assert git("rev-parse", f"{pull['head']}^") == base
            landed = pull["head"]
        else:
            tree = git("rev-parse", f"{base if style == 'wrong-tree' else pull['head']}^{{tree}}")
            parents = ["-p", pull["head"] if style == "wrong-parent" else base]
            if style == "merge":
                parents += ["-p", pull["head"]]
            landed = subprocess.check_output(
                ["git", "--git-dir", remote, "commit-tree", tree, *parents, "-m", f"{style} #{pull['id']}"],
                text=True, env={**os.environ, **identity}).strip()
        git("update-ref", f"refs/heads/{pull['base']}", landed, base)
        pull.update(state="merged", mergeCommit=landed)

    if request["operation"] == "inspect":
        for pull in pulls:
            if pull["coordinate"] == request["coordinate"] and pull["state"] == "open":
                if "humanMerge" in control:
                    merge(pull, control["humanMerge"])
                elif control.get("humanClose"):
                    pull["state"] = "closed"
    branch = request["branch"] or "trunk"
    revision = request["revision"] or git("rev-parse", f"refs/heads/{branch}")
    status = "success"
    if request["operation"] == "qualify" and not request["branch"] and "replaceGenerator" in control:
        pathlib.Path(control["replaceGenerator"]).write_text("#!/bin/sh\nexit 71\n")
    if request["operation"] == "qualify" and request["branch"]:
        if "expectedWorkflows" in control:
            assert request["workflows"] == control["expectedWorkflows"]
        phase = "candidate" if branch.startswith("maintenance/") else "adoption"
        status = control.get("coordinateStatus", {}).get(request["coordinate"], {}).get(
            phase, control.get(phase, "success"))
    checks = [
        dict(name=r["name"], authority=r["authority"], revision=revision,
             run="original-42", attempt=1, status="success")
        for r in request["requiredChecks"]
    ] if request["operation"] == "qualify" and status == "success" else []
    response = dict(
        schemaVersion=request["schemaVersion"], coordinate=request["coordinate"], repositoryId="42",
        defaultBranch=control.get("defaultBranch", "trunk"), revision=revision,
        remote=control.get("remotes", {}).get(request["coordinate"], control.get("remote", pathlib.Path(remote).as_uri())), policyDigest=control.get("policy", "b" * 64),
        requirements=[dict(name="test", authority="777")], status=status, checks=checks,
    )
    if request["operation"] == "admit":
        if "expectedAdmissionChecks" in control:
            assert request["requiredChecks"] == control["expectedAdmissionChecks"]
        if "expectedAdmissionBlobs" in control:
            assert {f["relative_path"]: f["content"] for f in request["plan"]["files"]} == control["expectedAdmissionBlobs"]
        response["status"] = control.get("admission", "success")
        response["checks"] = [dict(
            name="admission", authority=request["authority"], revision=revision,
            run=hashlib.sha256(json.dumps(request, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest(),
            attempt=1, status="success",
        )] if response["status"] == "success" else []
    if request["operation"] in ("propose", "merge", "close"):
        pull = next((p for p in pulls if p["id"] == request.get("change") or (
            not request.get("change") and p["coordinate"] == request["coordinate"]
            and p["branch"] == request["branch"] and p["state"] != "closed")), None)
        if pull is None and request["operation"] == "propose":
            pull = dict(id=str(len(pulls) + 1), coordinate=request["coordinate"], branch=request["branch"],
                        head=request["revision"], base=request["target"], state="open", mergeCommit=None)
            pulls.append(pull)
        if pull and pull["state"] == "open" and request["operation"] == "merge":
            assert pull["head"] == request["revision"]
            if control.get("coordinateStatus", {}).get(request["coordinate"], {}).get(
                    "merge", control.get("merge", "success")) == "success":
                merge(pull, control.get("mergeStyle", "merge"))
        if pull and pull["state"] == "open" and request["operation"] == "close":
            pull["state"] = "closed"
        response["status"] = "pending" if request["operation"] == "merge" and pull["state"] == "open" else "success"
        if pull:
            response["change"] = dict(id=pull["id"], state=pull["state"], head=pull["head"],
                                      base=pull["base"], mergeCommit=pull["mergeCommit"])
    if pulls:
        pulls_path.write_text(json.dumps(pulls))
    if request["operation"] == "inspect":
        if "replaceHelper" in control:
            pathlib.Path(control["replaceHelper"]).write_text("marker = 'unverified replacement'\n")
        control["inspections"] = control.get("inspections", 0) + 1
        if control.get("raceAtInspection") == control["inspections"]:
            git("update-ref", "refs/heads/trunk", control["raceRevision"], revision)
        pathlib.Path(control_path).write_text(json.dumps(control))
    return response


if __name__ == "__main__":
    control_path, remote, ready = sys.argv[1:]

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            length = int(self.headers["Content-Length"])
            assert 0 < length <= 16 * 1024 * 1024
            request = json.loads(self.rfile.read(length))
            body = json.dumps(respond(request, control_path, remote)).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    with HTTPServer(("127.0.0.1", 0), Handler) as server:
        pathlib.Path(ready).write_text(str(server.server_port))
        server.serve_forever()
