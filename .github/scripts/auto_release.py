"""Trusted fork automation. Never execute upstream code with a write token."""
import argparse
import base64
import configparser
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import time
import urllib.error
import urllib.request
import zipfile

UPSTREAM = "SaqibS/klatter"
DESTINATION = "kaveinthran/klatter"
PAYLOAD = {
    "manifest.ini", "synthDrivers/klatter/__init__.py",
    "synthDrivers/klatter/klatter.dll", "synthDrivers/klatter/cmudict-LICENSE.txt",
}
FIXED_ASSETS = {"klatter.nvda-addon", "SHA256SUMS.txt", "source.json", "LICENSE.md", "INSTALL.txt"}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def valid_sha(value):
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ValueError("Expected a full lowercase source commit SHA")
    return value


def api(path, missing_ok=False):
    url = "https://api.github.com/" + path.lstrip("/")
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "klatter-upstream-releases"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    for attempt in range(3):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if error.code == 404 and missing_ok:
                return None
            if error.code not in {429, 500, 502, 503, 504} or attempt == 2:
                raise
        except (TimeoutError, urllib.error.URLError):
            if attempt == 2:
                raise
        time.sleep(3 * (attempt + 1))


def run(*args, cwd=None, env=None, check=True):
    result = subprocess.run(args, cwd=cwd, env=env, check=False,
                            text=True, encoding="utf-8", errors="strict",
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and result.returncode:
        raise RuntimeError(f"{args[0]} failed ({result.returncode}): {result.stderr.strip()}")
    return result


def summary(text):
    print(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write(text + "\n")


def release_tag(source_sha):
    return "upstream-" + valid_sha(source_sha)


def release_complete(release, source_sha):
    if not release or release.get("draft"):
        return False
    if release.get("target_commitish") != source_sha:
        raise ValueError("Existing published release targets a different commit")
    assets = release.get("assets", [])
    names = {asset["name"] for asset in assets if asset.get("state") == "uploaded"}
    versioned = [name for name in names if re.fullmatch(
        r"klatter-[A-Za-z0-9][A-Za-z0-9.+_-]*\.nvda-addon", name)]
    if len(versioned) != 1 or names != FIXED_ASSETS | set(versioned) or len(assets) != len(names) or any(
            not re.fullmatch(r"sha256:[0-9a-f]{64}", asset.get("digest", "")) or
            asset.get("size", 0) <= 0 for asset in assets):
        raise ValueError("Existing published release is incomplete; refusing to replace it")
    return True


def verify_tag(source_sha, missing_ok=False):
    ref = api(f"repos/{DESTINATION}/git/ref/tags/{release_tag(source_sha)}", missing_ok=missing_ok)
    if ref is not None and (ref["object"].get("type") != "commit" or ref["object"].get("sha") != source_sha):
        raise ValueError("Release tag does not point to the expected source commit")


def sync_source(root, source_sha, now=None):
    """Merge source normally; retain all fork-owned .github files and history."""
    now = now or datetime.now(timezone.utc)
    run("git", "config", "user.name", "github-actions[bot]", cwd=root)
    run("git", "config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com", cwd=root)
    run("git", "fetch", "--no-tags", f"https://github.com/{UPSTREAM}.git", source_sha, cwd=root)
    ancestor = run("git", "merge-base", "--is-ancestor", source_sha, "HEAD", cwd=root, check=False)
    if ancestor.returncode not in (0, 1):
        raise RuntimeError("Could not inspect upstream ancestry")
    if ancestor.returncode == 1:
        result = run("git", "merge", "--no-commit", "--no-ff", source_sha, cwd=root, check=False)
        # This also removes any new upstream workflow files absent in our .github.
        run("git", "restore", "--source=HEAD", "--staged", "--worktree", "--", ".github", cwd=root)
        conflicts = run("git", "diff", "--name-only", "--diff-filter=U", cwd=root).stdout.strip()
        merge_head = run("git", "rev-parse", "--verify", "MERGE_HEAD", cwd=root, check=False)
        if conflicts or merge_head.returncode:
            run("git", "merge", "--abort", cwd=root, check=False)
            raise RuntimeError("Upstream merge failed or conflicts outside .github require review")
        if result.returncode not in (0, 1):
            run("git", "merge", "--abort", cwd=root, check=False)
            raise RuntimeError("Upstream merge failed")
        run("git", "commit", "--no-edit", cwd=root)
    status_file = root / ".github/upstream-status.json"
    previous = json.loads(status_file.read_text(encoding="utf-8")) if status_file.exists() else {}
    last = datetime.fromisoformat(previous["checked_at"]) if previous.get("checked_at") else None
    if last is None or now - last >= timedelta(days=28):
        status_file.write_text(json.dumps({"upstream": UPSTREAM, "source_sha": source_sha,
                                          "checked_at": now.isoformat()}, indent=2) + "\n", encoding="utf-8")
        run("git", "add", "--", ".github/upstream-status.json", cwd=root)
        run("git", "commit", "-m", "Record upstream check to keep scheduled builds active", cwd=root)
    # Authentication lives in this subprocess environment, never in an argument or URL.
    token = os.environ.get("GH_TOKEN", "")
    if not token:
        raise RuntimeError("A job-scoped GitHub token is required for source sync")
    env = os.environ.copy()
    env.update({"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.https://github.com/.extraheader",
                "GIT_CONFIG_VALUE_0": "AUTHORIZATION: basic " + base64.b64encode(
                    ("x-access-token:" + token).encode()).decode()})
    run("git", "push", "origin", "HEAD:refs/heads/main", cwd=root, env=env)


def check(root):
    commit = api(f"repos/{UPSTREAM}/commits/main")
    source_sha = valid_sha(commit["sha"])
    sync_source(root, source_sha)
    release = api(f"repos/{DESTINATION}/releases/tags/{release_tag(source_sha)}", missing_ok=True)
    needed = not release_complete(release, source_sha)
    if not needed:
        verify_tag(source_sha)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(f"sha={source_sha}\nbuild_needed={str(needed).lower()}\n")
    summary(f"Upstream commit: `{source_sha}`. " + ("Build and release required." if needed else
            "A complete release already exists; Windows build and publication skipped."))


def inspect_package(path):
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        if len(names) != len(PAYLOAD) or set(names) != PAYLOAD or archive.testzip() is not None:
            raise ValueError("Package payload or ZIP integrity changed; review required")
        for name in names:
            if not name.endswith(".dll"):
                text = archive.read(name).decode("utf-8", errors="strict")
                if "\ufffd" in text:
                    raise ValueError("Replacement character in package text")
        manifest = configparser.ConfigParser()
        manifest.read_string("[addon]\n" + archive.read("manifest.ini").decode("utf-8"))
        values = {key: value.strip('"') for key, value in manifest["addon"].items()}
        if values.get("name") != "klatter" or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+_-]{0,79}", values["version"]):
            raise ValueError("Invalid add-on identity/version")
        for key in ("minimumnvdaversion", "lasttestednvdaversion"):
            if not re.fullmatch(r"\d{4}\.\d+(?:\.\d+)?", values[key]):
                raise ValueError("Invalid NVDA compatibility declaration")
        dll = archive.read("synthDrivers/klatter/klatter.dll")
        if dll[:2] != b"MZ" or len(dll) < 64:
            raise ValueError("Missing PE DLL")
        offset = struct.unpack_from("<I", dll, 0x3C)[0]
        if offset + 24 > len(dll) or dll[offset:offset+4] != b"PE\0\0" or \
                struct.unpack_from("<H", dll, offset+4)[0] != 0x8664 or \
                not struct.unpack_from("<H", dll, offset+22)[0] & 0x2000:
            raise ValueError("Expected an x64 Windows DLL")
        return values


def package(source, output, source_sha):
    actual = run("git", "rev-parse", "HEAD", cwd=source).stdout.strip()
    if actual != valid_sha(source_sha):
        raise ValueError("Build source commit mismatch")
    candidates = list((source / "target").glob("*.nvda-addon"))
    if len(candidates) != 1:
        raise ValueError("Expected one freshly built add-on")
    original = candidates[0]
    values = inspect_package(original)
    version = values["version"]
    if original.name != f"klatter-{version}.nvda-addon":
        raise ValueError("Filename/version mismatch")
    with zipfile.ZipFile(original) as archive:
        for name in PAYLOAD:
            expected = (source / "data/cmudict/LICENSE.txt" if name.endswith("cmudict-LICENSE.txt")
                        else source / "nvda/addon" / name)
            if archive.read(name) != expected.read_bytes():
                raise ValueError("Packaged file does not match built source")
    output.mkdir(parents=True, exist_ok=False)
    shutil.copyfile(original, output / original.name)
    shutil.copyfile(original, output / "klatter.nvda-addon")
    shutil.copyfile(source / "LICENSE.md", output / "LICENSE.md")
    info = {"upstream": UPSTREAM, "source_sha": source_sha, "version": version,
            "minimum_nvda": values["minimumnvdaversion"], "last_tested_nvda": values["lasttestednvdaversion"],
            "package": original.name, "sha256": sha256(original), "architecture": "x86_64-pc-windows-msvc",
            "built_at": datetime.now(timezone.utc).isoformat(),
            "workflow_run": f"https://github.com/{DESTINATION}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}"}
    (output / "source.json").write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
    (output / "INSTALL.txt").write_text(
        f"Klatter {version} for NVDA\n\nRequires NVDA {info['minimum_nvda']} or later.\n"
        "Open klatter.nvda-addon while NVDA is running. Follow the installation prompts, restart NVDA "
        "when requested, then select Klatter in Speech settings.\n\n"
        f"Community build of https://github.com/{UPSTREAM}/commit/{source_sha}.\n"
        "Automated tests passed; live NVDA speech and installation have not been checked.\n"
        "The MIT licence is attached; the dictionary licence is inside the add-on.\n", encoding="utf-8")
    lines = [f"{sha256(path)}  {path.name}\n" for path in sorted(output.iterdir())]
    (output / "SHA256SUMS.txt").write_text("".join(lines), encoding="utf-8")
    validate_artifact(output, source_sha)
    summary(f"Validated Klatter {version}, source `{source_sha}`, SHA-256 `{info['sha256']}`.")


def validate_artifact(root, source_sha):
    valid_sha(source_sha)
    info = json.loads((root / "source.json").read_text(encoding="utf-8"))
    if info.get("source_sha") != source_sha or info.get("upstream") != UPSTREAM:
        raise ValueError("Artifact provenance mismatch")
    name = info.get("package", "")
    if not re.fullmatch(r"klatter-[A-Za-z0-9][A-Za-z0-9.+_-]{0,79}\.nvda-addon", name):
        raise ValueError("Invalid artifact filename")
    expected = FIXED_ASSETS | {name}
    entries = list(root.iterdir())
    if {p.name for p in entries} != expected or any(not p.is_file() or p.is_symlink() for p in entries):
        raise ValueError("Unexpected release files")
    values = inspect_package(root / name)
    if values["version"] != info.get("version") or name != f"klatter-{values['version']}.nvda-addon":
        raise ValueError("Artifact version mismatch")
    if info.get("minimum_nvda") != values["minimumnvdaversion"] or \
            info.get("last_tested_nvda") != values["lasttestednvdaversion"] or \
            info.get("architecture") != "x86_64-pc-windows-msvc":
        raise ValueError("Artifact compatibility metadata mismatch")
    if sha256(root / name) != info.get("sha256") or (root / name).read_bytes() != (root / "klatter.nvda-addon").read_bytes():
        raise ValueError("Artifact checksum or stable alias mismatch")
    expected_lines = [f"{sha256(path)}  {path.name}\n" for path in sorted(entries) if path.name != "SHA256SUMS.txt"]
    if (root / "SHA256SUMS.txt").read_text(encoding="utf-8") != "".join(expected_lines):
        raise ValueError("Checksum manifest mismatch")
    return info


def verify_upload(release, root):
    assets = release.get("assets", [])
    if {a["name"] for a in assets} != {p.name for p in root.iterdir()}:
        raise ValueError("Uploaded asset list mismatch")
    for asset in assets:
        path = root / asset["name"]
        if asset.get("state") != "uploaded" or asset.get("digest") != "sha256:" + sha256(path) or asset["size"] != path.stat().st_size:
            raise ValueError("Uploaded asset hash/size mismatch")


def publish(root, source_sha):
    info = validate_artifact(root, source_sha)
    tag = release_tag(source_sha)
    release_path = f"repos/{DESTINATION}/releases/tags/{tag}"
    release = api(release_path, missing_ok=True)
    if release_complete(release, source_sha):
        verify_tag(source_sha)
        summary("This source commit is already published; no assets changed.")
        return
    if release and release.get("target_commitish") != source_sha:
        raise ValueError("Draft release targets a different commit")
    verify_tag(source_sha, missing_ok=True)
    notes = (
        f"Community build of **Klatter {info['version']}** by Saqib Shaikh from "
        f"[SaqibS/klatter](https://github.com/{UPSTREAM}), exact source "
        f"[{source_sha[:12]}](https://github.com/{UPSTREAM}/commit/{source_sha}).\n\n"
        f"Download **klatter.nvda-addon** below. Requires **NVDA {info['minimum_nvda']} or later**. "
        "Open the file, follow installation prompts, restart NVDA when requested, then select Klatter in Speech settings.\n\n"
        "Rust workspace/all-target tests, NVDA driver tests and Python tools tests passed. "
        "ZIP integrity, source identity, payload, x64 DLL and checksums were verified. "
        "Driver tests use NVDA stand-ins; live NVDA speech and installation have not been checked.\n\n"
        f"[Build/test run](https://github.com/{DESTINATION}/actions/runs/{os.environ['GITHUB_RUN_ID']}). "
        f"SHA-256: `{info['sha256']}`.\n\n"
        "MIT licence, source provenance and SHA256SUMS.txt are attached. The CMU dictionary licence is inside the add-on. "
        "Upstream may change its code without changing the version number; the source commit identifies this build.\n"
    )
    notes_path = root.parent / "release-notes.md"
    notes_path.write_text(notes, encoding="utf-8")
    if not release:
        run("gh", "release", "create", tag, "--repo", DESTINATION, "--target", source_sha,
            "--title", f"Klatter {info['version']} for NVDA ({source_sha[:12]})", "--notes-file", str(notes_path), "--draft")
    run("gh", "release", "upload", tag, *[str(p) for p in sorted(root.iterdir())], "--repo", DESTINATION, "--clobber")
    verify_upload(api(release_path), root)
    run("gh", "release", "edit", tag, "--repo", DESTINATION, "--notes-file", str(notes_path), "--draft=false", "--latest")
    published = api(release_path)
    if published.get("draft") or published.get("target_commitish") != source_sha:
        raise ValueError("Published release identity mismatch")
    verify_upload(published, root)
    verify_tag(source_sha)
    # No authentication headers: check the same public download a user receives.
    download_url = f"https://github.com/{DESTINATION}/releases/download/{tag}/klatter.nvda-addon"
    for attempt in range(3):
        try:
            with urllib.request.urlopen(download_url, timeout=60) as response:
                downloaded = response.read()
            if hashlib.sha256(downloaded).hexdigest() != info["sha256"]:
                raise ValueError("Public download checksum mismatch")
            break
        except (urllib.error.URLError, TimeoutError):
            if attempt == 2:
                raise
            time.sleep(5 * (attempt + 1))
    summary(f"Published and verified: https://github.com/{DESTINATION}/releases/tag/{tag}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["check", "package", "publish"])
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--source", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if os.environ.get("GITHUB_REPOSITORY") != DESTINATION:
        raise ValueError("This automation is configured only for " + DESTINATION)
    if args.action == "check":
        check(args.root.resolve())
    elif args.action == "package":
        package(args.source.resolve(), args.output.resolve(), os.environ["SOURCE_SHA"])
    else:
        publish(args.root.resolve(), os.environ["SOURCE_SHA"])


if __name__ == "__main__":
    main()
