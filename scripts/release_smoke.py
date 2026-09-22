"""Exercise an extracted native release using only a loopback fixture."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from urllib.parse import urlsplit


def smoke(binary: Path, version: str) -> None:
    binary = binary.resolve()
    assert subprocess.check_output([binary, "--version"], text=True).strip() == f"hyfa {version}"
    help_text = subprocess.check_output([binary, "--help"], text=True)
    for command in ("auth", "skill", "create", "comment", "view", "next", "plan", "ready", "graph", "reconcile"):
        assert any(line.strip().startswith(f"{command} ") for line in help_text.splitlines()), command

    class Fixture(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            allowed = {"labels", "issues", "issues/events", "issues/comments"}
            path = urlsplit(self.path).path.removeprefix("/repos/release/smoke/")
            self.send_response(200 if path in allowed else 404)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b"[]")

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            valid = self.path == "/graphql" and b"IssueInventoryCount" in body
            self.send_response(200 if valid else 404)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"data":{"repository":{"issues":{"totalCount":0}}}}')

    with tempfile.TemporaryDirectory(prefix="hyfa-release-smoke-") as directory:
        env = dict(os.environ)
        # Prevent discovery of the operator's keychain or any real API endpoint.
        env.update(PATH="", GH_TOKEN="fixture-only", HYFA_STATE_DIR=directory,
                   HYFA_NO_KEYRING="1",
                   NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
        skill_project = Path(directory) / "skill-project"
        skill_project.mkdir()
        install_args = [binary, "skill", "install", "--project-root", str(skill_project)]
        installed = subprocess.run(install_args, env=env, cwd=skill_project,
                                   capture_output=True, text=True, timeout=30)
        if installed.returncode:
            raise RuntimeError(f"Embedded skill installation failed: {installed.stderr}")
        skill = skill_project / ".agents/skills/hyfa/SKILL.md"
        assert skill.is_file() and not skill.is_symlink()
        skill_payload = skill.read_bytes()
        assert b"name: hyfa" in skill_payload
        repeated = subprocess.run(install_args, env=env, cwd=skill_project,
                                  capture_output=True, text=True, timeout=30)
        assert repeated.returncode != 0
        assert skill.read_bytes() == skill_payload
        server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        env["HYFA_GITHUB_API_URL"] = f"http://127.0.0.1:{server.server_port}"
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()

        def run(*args):
            output = subprocess.run([binary, *args, "--json"], env=env, capture_output=True, text=True, timeout=30)
            if output.returncode:
                raise RuntimeError(f"Smoke command {args} failed: {output.stderr}")
            return json.loads(output.stdout)

        try:
            synced = run("sync", "--repo", "release/smoke")
            assert synced["repository"] == "release/smoke"
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
        env.pop("GH_TOKEN")
        created = run("create", "--repo", "release/smoke", "--title", "Ship a verified release")
        assert created["pending"] is True
        draft = created["draft"]["key"]
        priority = run("update", draft, "--priority", "p1")
        assert priority["pending"] is True and "number" not in priority["issue"]
        viewed = run("view", draft, "--offline")
        assert viewed["issue"]["priority"]["value"] == "p1"
        assert viewed["issue"]["relationships_complete"] is True
        scoped = run("ready", "--repo", "release/smoke", "--label", "priority:p1")
        assert [issue["key"] for issue in scoped["issues"]] == [draft]
        excluded = run("next", "--repo", "release/smoke", "--exclude-label", "priority:p1")
        assert excluded["recommendation"] is None
        ranked = run("next", "--repo", "release/smoke")
        assert ranked["source"] == "local_fallback"
        assert ranked["recommendation"]["first_issue"]["key"] == draft
        planned = run("plan", "--repo", "release/smoke")
        assert planned["repository"] == "release/smoke"
        assert planned["decision"]["pending"] is True
        assert planned["decision"]["recommendation"]["first_issue"]["key"] == draft
        site = Path(directory) / "graph"
        run("graph", "--repo", "release/smoke", "--output", str(site))
        assert (site / "index.html").is_file()
        assert (site / "graph.json").is_file()
        parent = run("create", "--repo", "release/smoke", "--title", "Release plan")["draft"]["key"]
        run("sub-issue", parent, "--add", draft)
        children = run("ready", "--repo", "release/smoke", "--children-of", parent)
        assert [issue["key"] for issue in children["issues"]] == [draft]
    print("Extracted binary passed version, bundled skill, local sync, scoped selection, Draft priority, Issue view, next, plan, and graph checks.")
