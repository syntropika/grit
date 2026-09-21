"""Regression tests for release identity and complete, trustworthy asset sets."""

import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import release


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
            patch.dict(os.environ, GH_TOKEN="fixture-token"),
            patch.object(release, "urlopen", side_effect=[io.BytesIO(json.dumps(first_page).encode()),
                                                         io.BytesIO(json.dumps([draft]).encode())]) as request,
        ):
            self.assertEqual(release.existing_release("release/smoke", "v1.0.0"), draft)
            self.assertTrue(request.call_args.args[0].full_url.endswith("page=2"))

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
        with (
            patch.dict(os.environ, GITHUB_EVENT_NAME="push", GITHUB_REF=f"refs/tags/v{self.version}",
                       GITHUB_REPOSITORY="release/smoke"),
            patch.object(release, "identity", return_value=(self.version, self.commit)),
            patch.object(release, "command", return_value=self.commit) as command,
            patch.object(release, "verify_archives", return_value=[]),
            patch.object(release, "existing_release", return_value={"draft": False}),
        ):
            with self.assertRaisesRegex(ValueError, "refusing to replace"):
                release.publish(self.output)
            self.assertEqual(command.call_count, 1)
            self.assertEqual(command.call_args.args[0], "git")

    def test_failed_upload_readback_leaves_release_as_draft(self):
        asset = self.output / "fixture.tar.gz"
        asset.write_bytes(b"verified archive")
        calls = []

        def command(*args):
            calls.append(args)
            if args[:3] == ("gh", "release", "download"):
                directory = Path(args[args.index("--dir") + 1])
                directory.mkdir()
                (directory / asset.name).write_bytes(b"truncated upload")
            return self.commit

        with (
            patch.dict(os.environ, GITHUB_EVENT_NAME="push", GITHUB_REF=f"refs/tags/v{self.version}",
                       GITHUB_REPOSITORY="release/smoke"),
            patch.object(release, "identity", return_value=(self.version, self.commit)),
            patch.object(release, "command", side_effect=command),
            patch.object(release, "verify_archives", return_value=[asset]),
            patch.object(release, "existing_release", side_effect=[None, {
                "draft": True, "assets": [{"name": asset.name}],
            }]),
        ):
            with self.assertRaisesRegex(ValueError, "checksum verification"):
                release.publish(self.output)
        self.assertTrue(any(call[:3] == ("gh", "release", "create") and "--draft" in call for call in calls))
        self.assertFalse(any(call[:3] == ("gh", "release", "edit") for call in calls))


if __name__ == "__main__":
    unittest.main()
