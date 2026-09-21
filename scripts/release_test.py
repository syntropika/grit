"""Regression tests for release identity and complete, trustworthy asset sets."""

import io
import json
import os
from contextlib import contextmanager
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlsplit
from urllib.request import Request

import release


class ReleaseAPIFixture:
    def __init__(self, version, commit):
        self.tag = f"v{version}"
        self.commit = commit
        self.events = []
        self.existing = False
        self.draft = None
        self.assets = {}
        self.corrupt_download = False
        self.fail_upload_at = None

    def request(self, url, method="GET", data=None, content_type="application/json", binary=False):
        parsed = urlsplit(url)
        path = parsed.path
        self.events.append((method, path))
        if "/git/ref/tags/" in path:
            result = {"object": {"type": "commit", "sha": self.commit}}
        elif method == "GET" and path.endswith("/releases"):
            result = [{"tag_name": self.tag, "draft": False}] if self.existing else []
        elif method == "POST" and path.endswith("/releases"):
            self.draft = json.loads(data)
            assert self.draft["draft"] is True
            self.draft["id"] = 17
            result = self.draft
        elif method == "POST" and path.endswith("/releases/17/assets"):
            assert parsed.hostname == "uploads.github.com"
            if self.fail_upload_at == len(self.assets) + 1:
                raise RuntimeError("Fixture upload failure")
            name = parse_qs(parsed.query)["name"][0]
            asset_id = len(self.assets) + 1
            metadata = {"name": name, "id": asset_id, "size": len(data), "state": "uploaded"}
            self.assets[asset_id] = (metadata, data)
            result = metadata
        elif method == "GET" and path.endswith("/releases/17"):
            result = {**self.draft, "assets": [metadata for metadata, _ in self.assets.values()]}
        elif method == "GET" and "/releases/assets/" in path:
            assert binary is True
            data = self.assets[int(path.rsplit("/", 1)[1])][1]
            return b"truncated upload" if self.corrupt_download else data
        elif method == "PATCH" and path.endswith("/releases/17"):
            self.draft.update(json.loads(data))
            result = self.draft
        else:
            raise AssertionError(f"Unexpected fixture operation: {method} {path}")
        return json.dumps(result).encode()


class ReleaseGuards(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.output = Path(self.temporary.name)
        self.version, self.commit = release.identity()

    def archive(self, target, *, commit=None, unsafe=False):
        name = release.archive_name(self.version, target)
        metadata = {
            "version": self.version,
            "commit": commit or self.commit,
            "target": target,
            "rust": release.RUST_VERSION,
            "cargo_lock_sha256": release.sha256(release.ROOT / "Cargo.lock"),
            "minimum_platform": "glibc 2.35" if "linux" in target else "macOS 13.0",
        }
        with tarfile.open(self.output / f"{name}.tar.gz", "w:gz") as bundle:
            for filename in release.ARCHIVE_FILES:
                data = json.dumps(metadata).encode() if filename == "release.json" else b"fixture"
                info = tarfile.TarInfo(f"{name}/{filename}")
                info.mode = 0o755 if filename == "grit" else 0o644
                info.size = len(data)
                if unsafe and filename == "grit":
                    info.type = tarfile.SYMTYPE
                    info.linkname = "/unexpected/binary"
                    info.size = 0
                bundle.addfile(info, io.BytesIO(data))

    def test_tag_version_mismatch_is_rejected(self):
        with patch.dict(os.environ, GITHUB_REF="refs/tags/v999.0.0"):
            with self.assertRaisesRegex(ValueError, "does not match package version"):
                release.identity()

    def test_release_identity_uses_the_manifest_package_name(self):
        (self.output / "Cargo.toml").write_text('[package]\nname = "registry-name"\nversion = "1.2.3"\n')
        (self.output / "Cargo.lock").write_text('[[package]]\nname = "registry-name"\nversion = "1.2.3"\n')
        with (patch.object(release, "ROOT", self.output),
              patch.object(release, "command", return_value=self.commit),
              patch.dict(os.environ, GITHUB_REF="", GITHUB_SHA=self.commit)):
            self.assertEqual(release.identity(), ("1.2.3", self.commit))
            (self.output / "Cargo.lock").write_text('[[package]]\nname = "registry-name"\nversion = "1.2.2"\n')
            with self.assertRaisesRegex(ValueError, "versions do not match"):
                release.identity()

    def test_linux_binary_cannot_claim_an_older_glibc_floor(self):
        with patch.object(release, "command", return_value="Name: GLIBC_2.34\nName: GLIBC_2.39"):
            with self.assertRaisesRegex(ValueError, "requires glibc 2.39"):
                release.platform_requirement(Path("grit"), "x86_64-unknown-linux-gnu")

    def test_linux_binary_with_compatible_symbols_is_accepted(self):
        with patch.object(release, "command", return_value="Name: GLIBC_2.9\nName: GLIBC_2.34"):
            self.assertEqual(release.platform_requirement(Path("grit"), "aarch64-unknown-linux-gnu"), "glibc 2.34")

    def test_macos_deployment_target_is_checked_in_binary(self):
        for minimum, allowed in (("13.0", True), ("13.0.0", True), ("13.0.1", False), ("14.0", False)):
            with patch.object(release, "command", return_value=f"cmd LC_BUILD_VERSION\ncmdsize 32\nplatform 1\nminos {minimum}\nsdk 15.5"):
                if allowed:
                    self.assertEqual(release.platform_requirement(Path("grit"), "aarch64-apple-darwin"), f"macOS {minimum}")
                else:
                    with self.assertRaisesRegex(ValueError, f"requires macOS {minimum}"):
                        release.platform_requirement(Path("grit"), "aarch64-apple-darwin")

    def test_macos_linker_tool_version_is_not_a_deployment_requirement(self):
        output = """Load command 10
      cmd LC_BUILD_VERSION
  cmdsize 32
 platform 1
    minos 13.0
      sdk 15.5
   ntools 1
     tool 3
  version 1167.5
Load command 11
      cmd LC_SOURCE_VERSION
  cmdsize 16
  version 0.0
"""
        with patch.object(release, "command", return_value=output):
            self.assertEqual(release.platform_requirement(Path("grit"), "aarch64-apple-darwin"), "macOS 13.0")

    def test_legacy_macos_minimum_version_command_is_supported(self):
        output = "cmd LC_VERSION_MIN_MACOSX\ncmdsize 16\nversion 13.0\nsdk 15.5"
        with patch.object(release, "command", return_value=output):
            self.assertEqual(release.platform_requirement(Path("grit"), "x86_64-apple-darwin"), "macOS 13.0")

    def test_archive_links_keep_bundled_guides_and_pin_unbundled_sources(self):
        (self.output / "README.md").write_text("[Usage](docs/usage.md)\n[Contributing](CONTRIBUTING.md)\n")
        with patch.object(release, "ROOT", self.output):
            text = release.bundled_document("README.md", self.commit).decode()
        self.assertIn("](docs/usage.md)", text)
        self.assertIn(f"](https://github.com/syntropika/grit/blob/{self.commit}/CONTRIBUTING.md)", text)

    def test_existing_draft_is_found_on_later_release_page(self):
        first_page = [{"tag_name": f"v0.0.{number}", "draft": False} for number in range(100)]
        draft = {"tag_name": "v1.0.0", "draft": True}
        with (
            patch.object(release, "github_request", side_effect=[json.dumps(first_page).encode(),
                                                                 json.dumps([draft]).encode()]) as request,
        ):
            self.assertEqual(release.existing_release("release/smoke", "v1.0.0"), draft)
            self.assertTrue(request.call_args.args[0].endswith("page=2"))

    def test_unexpected_checkout_is_rejected(self):
        with patch.dict(os.environ, GITHUB_SHA="0" * 40):
            with self.assertRaisesRegex(ValueError, "Checkout does not match"):
                release.identity()

    def test_partial_matrix_is_rejected_before_creating_checksums(self):
        self.archive(release.TARGETS[0])
        with self.assertRaisesRegex(ValueError, "exactly all four targets"):
            release.verify_archives(self.output, self.version, self.commit)
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_mixed_commit_matrix_is_rejected(self):
        for index, target in enumerate(release.TARGETS):
            self.archive(target, commit="0" * 40 if index == 2 else self.commit)
        with self.assertRaisesRegex(ValueError, "mismatched commit"):
            release.verify_archives(self.output, self.version, self.commit)

    def test_symlink_payload_is_rejected(self):
        for index, target in enumerate(release.TARGETS):
            self.archive(target, unsafe=index == 0)
        with self.assertRaisesRegex(ValueError, "link or non-regular"):
            release.verify_archives(self.output, self.version, self.commit)

    def test_complete_verified_matrix_generates_deterministic_checksums(self):
        for target in release.TARGETS:
            self.archive(target)
        assets = release.verify_archives(self.output, self.version, self.commit)
        self.assertEqual(len(assets), 5)
        checksums = (self.output / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(checksums, sorted(checksums, key=lambda line: line.split("  ")[1]))
        for line in checksums:
            digest, name = line.split("  ")
            self.assertEqual(digest, release.sha256(self.output / name))

    def test_workflow_dispatch_cannot_publish(self):
        with patch.dict(os.environ, GITHUB_EVENT_NAME="workflow_dispatch"):
            with self.assertRaisesRegex(ValueError, "matching tag push"):
                release.publish(self.output)

    def test_existing_release_cannot_be_overwritten(self):
        with self.publication_fixture() as api:
            api.existing = True
            with self.assertRaisesRegex(ValueError, "refusing to replace"):
                release.publish(self.output)
            self.assertTrue(all(method == "GET" for method, _ in api.events))

    def test_failed_upload_readback_leaves_release_as_draft(self):
        with self.publication_fixture() as api:
            api.corrupt_download = True
            with self.assertRaisesRegex(ValueError, "checksum verification"):
                release.publish(self.output)
            self.assertIs(api.draft["draft"], True)
            self.assertFalse(any(method == "PATCH" for method, _ in api.events))

    @contextmanager
    def publication_fixture(self):
        for target in release.TARGETS:
            self.archive(target)
        api = ReleaseAPIFixture(self.version, self.commit)
        with (
            patch.dict(os.environ, GITHUB_EVENT_NAME="push", GITHUB_REF=f"refs/tags/v{self.version}",
                       GITHUB_REPOSITORY="release/smoke"),
            patch.object(release, "identity", return_value=(self.version, self.commit)),
            patch.object(release, "command", return_value=self.commit),
            patch.object(release, "github_request", side_effect=api.request),
        ):
            yield api

    def test_complete_rest_publication_verifies_all_assets_before_publish(self):
        with self.publication_fixture() as api:
            release.publish(self.output)
        self.assertIs(api.draft["draft"], False)
        self.assertEqual(api.draft["target_commitish"], self.commit)
        self.assertEqual(api.draft["make_latest"], "true")
        self.assertEqual(len(api.assets), 5)
        self.assertEqual(sum("/releases/assets/" in path for _, path in api.events), 5)
        self.assertEqual(sum("/git/ref/tags/" in path for _, path in api.events), 2)
        self.assertEqual(api.events[-1], ("PATCH", "/repos/release/smoke/releases/17"))

    def test_upload_failure_leaves_an_unpublished_partial_draft(self):
        with self.publication_fixture() as api:
            api.fail_upload_at = 3
            with self.assertRaisesRegex(RuntimeError, "Fixture upload failure"):
                release.publish(self.output)
        self.assertEqual(len(api.assets), 2)
        self.assertIs(api.draft["draft"], True)
        self.assertFalse(any(method == "PATCH" for method, _ in api.events))

    def test_remote_tag_mismatch_prevents_release_creation(self):
        with self.publication_fixture() as api:
            api.commit = "0" * 40
            with self.assertRaisesRegex(ValueError, "remote release tag"):
                release.publish(self.output)
        self.assertIsNone(api.draft)
        self.assertTrue(all(method == "GET" for method, _ in api.events))

    def test_annotated_tag_resolves_to_the_tested_commit(self):
        with patch.object(release, "github_api", side_effect=[
            {"object": {"type": "tag", "sha": "1" * 40}},
            {"object": {"type": "commit", "sha": self.commit}},
        ]) as request:
            release.verify_remote_tag("release/smoke", "v0.1.0", self.commit)
        self.assertEqual(request.call_args.args[0], f"/repos/release/smoke/git/tags/{'1' * 40}")

    def test_asset_redirect_strips_authorization_across_hosts_and_redirect_chain(self):
        handler = release.ReleaseRedirectHandler()
        initial = Request("https://api.github.com/repos/release/smoke/releases/assets/1",
                          headers={"Authorization": "Bearer fixture-secret", "Accept": "application/octet-stream"})
        first = handler.redirect_request(initial, None, 302, "Found", {},
                                         "https://release-assets.githubusercontent.com/asset?signature=fixture")
        self.assertIsNone(first.get_header("Authorization"))
        self.assertEqual(first.get_header("Accept"), "application/octet-stream")
        second = handler.redirect_request(first, None, 302, "Found", {},
                                          "https://objects.githubusercontent.com/final-asset")
        self.assertIsNone(second.get_header("Authorization"))

    def test_same_api_host_redirect_preserves_authentication(self):
        request = Request("https://api.github.com/old", headers={"Authorization": "Bearer fixture-secret"})
        redirected = release.ReleaseRedirectHandler().redirect_request(
            request, None, 302, "Found", {}, "https://api.github.com/new")
        self.assertEqual(redirected.get_header("Authorization"), "Bearer fixture-secret")

    def test_release_transport_rejects_unexpected_hosts_and_plain_http(self):
        for url in ("http://api.github.com/repos/x/y", "https://unexpected.invalid/upload",
                    "https://fixture-secret@api.github.com/repos/x/y", "https://api.github.com:8443/repos/x/y"):
            with self.subTest(url=url), patch.object(release, "build_opener") as opener:
                with self.assertRaisesRegex(ValueError, "expected HTTPS"):
                    release.github_request(url)
                opener.assert_not_called()

    def test_redirects_reject_plain_http_and_mutating_requests(self):
        handler = release.ReleaseRedirectHandler()
        request = Request("https://api.github.com/asset", headers={"Authorization": "Bearer fixture-secret"})
        with self.assertRaisesRegex(ValueError, "expected HTTPS"):
            handler.redirect_request(request, None, 302, "Found", {}, "http://release-assets.githubusercontent.com/asset")
        upload = Request("https://uploads.github.com/asset", method="POST", data=b"archive")
        with self.assertRaisesRegex(ValueError, "mutations cannot follow"):
            handler.redirect_request(upload, None, 303, "See Other", {}, "https://api.github.com/other")

    def test_api_errors_never_include_token_or_signed_url(self):
        error = HTTPError("https://api.github.com/asset?secret=fixture-secret", 403,
                          "fixture-secret", {}, io.BytesIO(b"fixture-secret"))
        with patch.dict(os.environ, GITHUB_TOKEN="fixture-secret"), patch.object(release, "build_opener") as opener:
            opener.return_value.open.side_effect = error
            with self.assertRaisesRegex(RuntimeError, "HTTP 403") as raised:
                release.github_request("https://api.github.com/repos/release/smoke/releases")
            self.assertNotIn("fixture-secret", str(raised.exception))
            request = opener.return_value.open.call_args.args[0]
            self.assertEqual(request.get_header("Authorization"), "Bearer fixture-secret")


if __name__ == "__main__":
    unittest.main()
