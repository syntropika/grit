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
    assert subprocess.check_output([binary, "--version"], text=True).strip() == f"grit {version}"
    help_text = subprocess.check_output([binary, "--help"], text=True)
    for command in ("create", "comment", "next", "plan", "ready", "graph", "reconcile"):
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

    with tempfile.TemporaryDirectory(prefix="grit-release-smoke-") as directory:
        env = dict(os.environ)
        # Prevent discovery of the operator's gh session or any real API endpoint.
        env.update(PATH="", GH_TOKEN="fixture-only", GRIT_STATE_DIR=directory,
                   NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
        server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        env["GRIT_GITHUB_API_URL"] = f"http://127.0.0.1:{server.server_port}"
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
    print("Extracted binary passed version, command, local sync, offline Draft, next, plan, and graph checks.")
