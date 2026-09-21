"""Build deterministic release archives and publish only a complete verified set."""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import subprocess
import tarfile
import tempfile
import tomllib
from urllib.request import Request, urlopen

from release_smoke import smoke

ROOT = Path(__file__).resolve().parent.parent
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)
RUST_VERSION = "1.94.0"
ARCHIVE_FILES = ("grit", "LICENSE", "README.md", "CHANGELOG.md", "docs/installation.md", "docs/usage.md", "release.json")


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def identity():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    package = [p for p in lock["package"] if p["name"] == "grit" and "source" not in p]
    if len(package) != 1 or package[0]["version"] != version:
        raise ValueError("Cargo.toml and Cargo.lock versions do not match")
    ref = os.environ.get("GITHUB_REF", "")
    if ref.startswith("refs/tags/") and ref != f"refs/tags/v{version}":
        raise ValueError(f"Tag {ref} does not match package version {version}")
    commit = command("git", "rev-parse", "HEAD")
    if os.environ.get("GITHUB_SHA", commit) != commit:
        raise ValueError("Checkout does not match the workflow commit")
    return version, commit


def archive_name(version, target):
    return f"grit-v{version}-{target}"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def platform_requirement(binary, target):
    if "linux" in target:
        output = command("readelf", "--version-info", str(binary.resolve()))
        versions = re.findall(r"Name: GLIBC_([0-9.]+)", output)
        label, maximum = "glibc", (2, 35)
    else:
        output = command("otool", "-l", str(binary.resolve()))
        versions = []
        section = ""
        for line in output.splitlines():
            parts = line.split()
            if len(parts) == 2 and parts[0] == "cmd":
                section = parts[1]
            if section in {"LC_BUILD_VERSION", "LC_VERSION_MIN_MACOSX"} and len(parts) == 2 and parts[0] in {"minos", "version"}:
                versions.append(parts[1])
        label, maximum = "macOS", (13, 0)
    if not versions:
        raise ValueError("Cannot determine the binary's minimum platform version")
    required = max(tuple(map(int, version.split("."))) for version in versions)
    width = max(len(required), len(maximum))
    if required + (0,) * (width - len(required)) > maximum + (0,) * (width - len(maximum)):
        raise ValueError(f"Binary requires {label} {'.'.join(map(str, required))}, above the supported {'.'.join(map(str, maximum))} floor")
    return f"{label} {'.'.join(map(str, required))}"


def bundled_document(filename, commit, bundled=ARCHIVE_FILES):
    document = (ROOT / filename).read_text()

    def source_link(match):
        target = match[2]
        if re.match(r"[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith(("#", "//")):
            return match[0]
        path, separator, fragment = target.partition("#")
        resolved = posixpath.normpath(str(PurePosixPath(filename).parent / path))
        if resolved in bundled:
            return match[0]
        return f"{match[1]}https://github.com/syntropika/grit/blob/{commit}/{resolved}{separator}{fragment}{match[3]}"

    return re.sub(r"(\[[^\]\n]+\]\()([^\s)]+)(\))", source_link, document).encode()


def changelog_notes(version, commit):
    changelog = bundled_document("CHANGELOG.md", commit, bundled=()).decode()
    section = re.search(rf"(?ms)^## {re.escape(version)}\s*\n(.*?)(?=^## |\Z)", changelog)
    if not section:
        raise ValueError(f"CHANGELOG.md does not contain a section for {version}")
    return section[1].strip()


def package(binary, target, output):
    version, commit = identity()
    changelog_notes(version, commit)
    if target not in TARGETS:
        raise ValueError(f"Unsupported release target: {target}")
    rust = command("rustc", f"+{RUST_VERSION}", "-vV")
    if f"host: {target}" not in rust.splitlines():
        raise ValueError("A release archive must be built and tested on its native target")
    required_platform = platform_requirement(binary, target)
    name = archive_name(version, target)
    metadata = {
        "version": version,
        "commit": commit,
        "target": target,
        "rust": RUST_VERSION,
        "cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
        "minimum_platform": "glibc 2.35" if "linux" in target else "macOS 13.0",
        "required_platform": required_platform,
    }
    files = {
        "grit": binary.read_bytes(),
        "LICENSE": (ROOT / "LICENSE").read_bytes(),
        "README.md": bundled_document("README.md", commit),
        "CHANGELOG.md": bundled_document("CHANGELOG.md", commit),
        "docs/installation.md": bundled_document("docs/installation.md", commit),
        "docs/usage.md": bundled_document("docs/usage.md", commit),
        "release.json": (json.dumps(metadata, indent=2, sort_keys=True) + "\n").encode(),
    }
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{name}.tar.gz"
    if archive.exists():
        raise ValueError(f"Refusing to overwrite {archive}")
    epoch = int(command("git", "show", "-s", "--format=%ct", "HEAD"))
    with archive.open("xb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(mode="w", fileobj=compressed, format=tarfile.USTAR_FORMAT) as bundle:
                for filename, data in sorted(files.items()):
                    info = tarfile.TarInfo(f"{name}/{filename}")
                    info.size = len(data)
                    info.mode = 0o755 if filename == "grit" else 0o644
                    info.mtime = epoch
                    bundle.addfile(info, io.BytesIO(data))
    with tempfile.TemporaryDirectory(prefix="grit-release-extract-") as directory:
        with tarfile.open(archive) as bundle:
            bundle.extractall(directory, filter="data")
        smoke(Path(directory) / name / "grit", version)
    print(f"Packaged and verified {archive}")


def verify_archives(output, version, commit):
    expected = {f"{archive_name(version, target)}.tar.gz" for target in TARGETS}
    actual = {p.name for p in output.glob("*.tar.gz")}
    if actual != expected:
        raise ValueError(f"Release must contain exactly all four targets; found {sorted(actual)}")
    for target in TARGETS:
        name = archive_name(version, target)
        with tarfile.open(output / f"{name}.tar.gz") as bundle:
            members = bundle.getmembers()
            required = {f"{name}/{part}" for part in ARCHIVE_FILES}
            if len(members) != len(required) or {m.name for m in members} != required:
                raise ValueError(f"Unexpected archive contents: {name}")
            if not all(m.isfile() for m in members):
                raise ValueError(f"Archive contains a link or non-regular file: {name}")
            if bundle.getmember(f"{name}/grit").mode != 0o755:
                raise ValueError(f"Binary is not executable: {name}")
            metadata = json.load(bundle.extractfile(f"{name}/release.json"))
            for key, value in {"version": version, "commit": commit, "target": target, "rust": RUST_VERSION,
                               "minimum_platform": "glibc 2.35" if "linux" in target else "macOS 13.0",
                               "cargo_lock_sha256": sha256(ROOT / "Cargo.lock")}.items():
                if metadata.get(key) != value:
                    raise ValueError(f"Archive {name} has mismatched {key}")
    checksums = output / "SHA256SUMS"
    checksums.write_text("".join(f"{sha256(output / name)}  {name}\n" for name in sorted(expected)))
    return [output / name for name in sorted(expected)] + [checksums]


def existing_release(repository, tag):
    # Listing with push access includes drafts; the tag endpoint is documented
    # for published releases and cannot be used to guard draft publication.
    page = 1
    while True:
        request = Request(f"https://api.github.com/repos/{repository}/releases?per_page=100&page={page}", headers={
            "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        })
        with urlopen(request, timeout=30) as response:
            releases = json.load(response)
        matches = [release for release in releases if release["tag_name"] == tag]
        if len(matches) > 1:
            raise ValueError(f"Multiple releases refer to {tag}; inspect them before publishing")
        if matches:
            return matches[0]
        if len(releases) < 100:
            return None
        page += 1


def publish(output):
    version, commit = identity()
    tag = f"v{version}"
    if os.environ.get("GITHUB_EVENT_NAME") != "push" or os.environ.get("GITHUB_REF") != f"refs/tags/{tag}":
        raise ValueError("Publishing is only permitted from a matching tag push")
    if command("git", "rev-parse", f"{tag}^{{commit}}") != commit:
        raise ValueError("The release tag no longer points to the tested commit")
    assets = verify_archives(output, version, commit)
    repository = os.environ["GITHUB_REPOSITORY"]
    if existing_release(repository, tag) is not None:
        raise ValueError("A release already exists; refusing to replace it or any of its assets")
    with tempfile.TemporaryDirectory(prefix="grit-release-publish-") as directory:
        notes = Path(directory) / "notes.md"
        notes.write_text(
            changelog_notes(version, commit) + "\n\n"
            "Download the archive for your platform and verify it against `SHA256SUMS` "
            "before extracting. Each archive includes the executable, user guides, README, MIT license, "
            "and the exact source commit in `release.json`.\n\n"
            "Linux binaries require glibc 2.35 or newer; macOS binaries require macOS 13 or newer. "
            "These macOS binaries are not Apple-notarized. Windows binaries are not provided.\n\n"
            f"[Installation and usage](https://github.com/{repository}/blob/{tag}/README.md) "
            f"· [Source commit](https://github.com/{repository}/commit/{commit})\n"
        )
        command("gh", "release", "create", tag, "--repo", repository, "--verify-tag", "--draft",
                "--title", f"Grit {tag}", "--notes-file", str(notes))
        command("gh", "release", "upload", tag, "--repo", repository, *(str(p) for p in assets))
        release = existing_release(repository, tag)
        if not release or not release["draft"] or {a["name"] for a in release["assets"]} != {p.name for p in assets}:
            raise ValueError("The draft release does not contain exactly the verified asset set")
        downloaded = Path(directory) / "downloaded"
        command("gh", "release", "download", tag, "--repo", repository, "--dir", str(downloaded))
        for asset in assets:
            if sha256(downloaded / asset.name) != sha256(asset):
                raise ValueError(f"Uploaded asset failed checksum verification: {asset.name}")
        # If upload or verification fails, the incomplete release remains a draft.
        command("gh", "release", "edit", tag, "--repo", repository, "--draft=false", "--latest")
    print(f"Published complete, verified release: https://github.com/{repository}/releases/tag/{tag}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check")
    package_parser = commands.add_parser("package")
    package_parser.add_argument("--target", required=True, choices=TARGETS)
    package_parser.add_argument("--binary", type=Path, required=True)
    package_parser.add_argument("--output", type=Path, required=True)
    publish_parser = commands.add_parser("publish")
    publish_parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "check":
        version, commit = identity()
        changelog_notes(version, commit)
        print("Release identity:", version, commit)
    elif args.command == "package":
        package(args.binary, args.target, args.output)
    else:
        publish(args.output)


if __name__ == "__main__":
    main()
