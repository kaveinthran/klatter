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
FIXED_ASSETS = {"klatter.nvda-addon", "SHA256SUMS.txt", "source.json", "LICENSE.md", "INSTALL.txt",
                "CHANGELOG.md", "CURRENT_FEATURES.md"}


def write_utf8(path, text):
    path.write_text(text, encoding="utf-8", newline="\n")


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


def find_release(source_sha):
    tag = release_tag(source_sha)
    release = api(f"repos/{DESTINATION}/releases/tags/{tag}", missing_ok=True)
    if release is None:
        # A draft's real tag may not exist yet; GitHub exposes it by ID/list,
        # while the tag endpoint can still return 404 to an authenticated caller.
        releases = api(f"repos/{DESTINATION}/releases?per_page=100")
        release = next((item for item in releases if item.get("tag_name") == tag), None)
    return release


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


def previous_source(releases):
    automated = [item for item in releases if not item.get("draft") and
                 item.get("tag_name", "").startswith("upstream-") and item.get("published_at")]
    latest = max(automated, key=lambda item: item["published_at"]) if automated else None
    return valid_sha(latest["target_commitish"]) if latest else ""


def community_version(source_sha, release, run_number, now=None):
    valid_sha(source_sha)
    if release and release.get("draft"):
        if release.get("target_commitish") != source_sha:
            raise ValueError("Draft release source mismatch")
        match = re.fullmatch(r"Klatter ([0-9]+\.[0-9]+\.[0-9]+) for NVDA \(([0-9a-f]{12})\)", release.get("name", ""))
        if not match or match[2] != source_sha[:12]:
            raise ValueError("Cannot recover the community version from this draft")
        return match[1]
    if not re.fullmatch(r"[1-9][0-9]*", run_number):
        raise ValueError("Expected an increasing GitHub workflow run number")
    now = now or datetime.now(timezone.utc)
    return f"{now.year}.{now.month * 100 + now.day}.{int(run_number)}"


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
        base = run("git", "merge-base", "HEAD", source_sha, cwd=root, check=False)
        if base.returncode == 1:
            # Upstream replaced its entire history. Preserve our history and
            # automation, but take the exact new source snapshot, including deletions.
            summary("Upstream history was replaced; importing the exact source snapshot.")
            result = run("git", "merge", "--allow-unrelated-histories", "-s", "ours",
                         "--no-commit", "--no-ff", source_sha, cwd=root, check=False)
            if result.returncode:
                run("git", "merge", "--abort", cwd=root, check=False)
                raise RuntimeError("Could not join replaced upstream history")
            run("git", "restore", "--source=" + source_sha, "--staged", "--worktree",
                "--", ".", ":(exclude).github", cwd=root)
        elif base.returncode == 0:
            result = run("git", "merge", "--no-commit", "--no-ff", source_sha, cwd=root, check=False)
        else:
            raise RuntimeError("Could not inspect upstream merge base")
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


def upstream_head(root):
    """Read the advertised branch ref, rather than a cached commit response."""
    result = run("git", "ls-remote", "--exit-code", "--refs",
                 f"https://github.com/{UPSTREAM}.git", "refs/heads/main", cwd=root)
    lines = result.stdout.strip().splitlines()
    if len(lines) != 1:
        raise ValueError("Expected exactly one advertised upstream main ref")
    fields = lines[0].split()
    if len(fields) != 2 or fields[1] != "refs/heads/main":
        raise ValueError("Unexpected advertised upstream ref")
    return valid_sha(fields[0])


def check(root):
    source_sha = upstream_head(root)
    sync_source(root, source_sha)
    release = find_release(source_sha)
    needed = not release_complete(release, source_sha)
    if not needed:
        verify_tag(source_sha)
    published = api(f"repos/{DESTINATION}/releases?per_page=100")
    previous_sha = previous_source(published)
    version = community_version(source_sha, release, os.environ["GITHUB_RUN_NUMBER"]) if needed else ""
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(f"sha={source_sha}\nbuild_needed={str(needed).lower()}\nprevious_sha={previous_sha}\ncommunity_version={version}\n")
    summary(f"Upstream commit: `{source_sha}`. " + ("Build and release required." if needed else
            "A complete release already exists; Windows build and publication skipped."))


def prepare(source, source_sha, previous_sha, run_number, now=None, version_override=None):
    if run("git", "rev-parse", "HEAD", cwd=source).stdout.strip() != valid_sha(source_sha):
        raise ValueError("Preparation source mismatch")
    if previous_sha:
        valid_sha(previous_sha)
    version = version_override or community_version(source_sha, None, run_number, now)
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Invalid expected community version")
    path = source / "nvda/addon/manifest.ini"
    original = path.read_text(encoding="utf-8")
    manifest = configparser.ConfigParser()
    manifest.read_string("[addon]\n" + original)
    upstream_version = manifest["addon"]["version"].strip('"')
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+_-]{0,79}", upstream_version):
        raise ValueError("Invalid upstream version")
    summary_value = manifest["addon"]["summary"].strip('"') + " (community build)"
    amended, count = re.subn(r"(?m)^version\s*=.*$", f"version = {version}", original)
    if count != 1:
        raise ValueError("Expected exactly one upstream version declaration")
    amended = re.sub(r"(?m)^summary\s*=.*$", f'summary = "{summary_value}"', amended)
    if not re.search(r"(?m)^url\s*=", amended):
        amended = amended.rstrip() + f'\nurl = "https://github.com/{DESTINATION}/releases/latest"\n'
    write_utf8(path, amended)
    write_utf8(source / ".community-build.json", json.dumps({
        "upstream_version": upstream_version, "version": version, "source_sha": source_sha,
        "previous_sha": previous_sha}, indent=2) + "\n")
    summary(f"Community version {version}; upstream version {upstream_version}; source `{source_sha}`.")


def changelog(source, source_sha, previous_sha, version, upstream_version):
    revision = source_sha
    baseline = "First automated build: the full upstream history is included below."
    if previous_sha:
        previous = run("git", "cat-file", "-e", previous_sha + "^{commit}", cwd=source, check=False)
        result = (run("git", "merge-base", "--is-ancestor", previous_sha, source_sha, cwd=source, check=False)
                  if previous.returncode == 0 else subprocess.CompletedProcess([], 1))
        if result.returncode == 0:
            revision = f"{previous_sha}..{source_sha}"
            baseline = f"Changes since the previous successful source build `{previous_sha}`."
        elif result.returncode == 1:
            baseline = "Upstream history diverged from the previous build; the full current history is included."
        else:
            raise RuntimeError("Cannot establish changelog baseline")
    raw = run("git", "log", "--reverse", "--topo-order", "--format=%H%x00%cI%x00%B%x00", revision, cwd=source).stdout
    parts = raw.split("\x00")
    if not parts[-1].strip():
        parts.pop()
    if len(parts) % 3:
        raise ValueError("Unexpected git history format")
    commits = []
    sections = [f"# Klatter community build {version}\n\nUpstream version: {upstream_version}. "
                f"Source: [{source_sha}](https://github.com/{UPSTREAM}/commit/{source_sha}).\n\n{baseline}\n\n"
                "Current capabilities and limitations are documented in CURRENT_FEATURES.md, a complete snapshot "
                "of the upstream README for this source. Commit descriptions below are the upstream author's "
                "messages; links open the complete code changes.\n"]
    if previous_sha:
        sections.append(f"\n[Complete source comparison](https://github.com/{UPSTREAM}/compare/{previous_sha}...{source_sha})\n")
    for index in range(0, len(parts), 3):
        commit_sha, timestamp, message = parts[index].strip(), parts[index+1].strip(), parts[index+2].rstrip()
        valid_sha(commit_sha)
        subject = message.splitlines()[0] if message else "(no commit message)"
        commits.append({"sha": commit_sha, "subject": subject})
        quoted = "\n".join("> " + line for line in message.splitlines())
        sections.append(f"\n## {subject}\n\n[{commit_sha}](https://github.com/{UPSTREAM}/commit/{commit_sha}) "
                        f"- {timestamp}\n\n{quoted}\n")
    sections.insert(1, f"\nIncluded commits: **{len(commits)}**. No commit messages are truncated.\n")
    return "".join(sections), commits


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
    context = json.loads((source / ".community-build.json").read_text(encoding="utf-8"))
    if context["source_sha"] != source_sha or context["version"] != version:
        raise ValueError("Community build context mismatch")
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
            "upstream_version": context["upstream_version"], "previous_sha": context["previous_sha"],
            "minimum_nvda": values["minimumnvdaversion"], "last_tested_nvda": values["lasttestednvdaversion"],
            "package": original.name, "sha256": sha256(original), "architecture": "x86_64-pc-windows-msvc",
            "built_at": datetime.now(timezone.utc).isoformat(),
            "workflow_run": f"https://github.com/{DESTINATION}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}"}
    changes, commits = changelog(source, source_sha, context["previous_sha"], version, context["upstream_version"])
    info["commit_count"] = len(commits)
    write_utf8(output / "CHANGELOG.md", changes)
    readme = next((p for p in source.iterdir() if p.name.lower() == "readme.md"), None)
    if readme is None:
        raise ValueError("Upstream README is missing")
    write_utf8(output / "CURRENT_FEATURES.md", f"# Current upstream documentation\n\n"
               f"Community build {version}, upstream version {context['upstream_version']}, source `{source_sha}`.\n\n"
               "The following is the upstream README at that exact commit. References to versions/building "
               "describe upstream; this distribution changes only NVDA packaging metadata and supplies release documentation.\n\n"
               + readme.read_text(encoding="utf-8"))
    write_utf8(output / "source.json", json.dumps(info, indent=2) + "\n")
    write_utf8(output / "INSTALL.txt",
        f"Klatter {version} for NVDA\n\nRequires NVDA {info['minimum_nvda']} or later.\n"
        "Open klatter.nvda-addon while NVDA is running. Follow the installation prompts, restart NVDA "
        "when requested, then select Klatter in Speech settings.\n\n"
        f"Community build of https://github.com/{UPSTREAM}/commit/{source_sha}.\n"
        "Automated tests passed; live NVDA speech and installation have not been checked.\n"
        "The MIT licence is attached; the dictionary licence is inside the add-on.\n"
        f"Upstream version: {context['upstream_version']}. The community version increases for store updates.\n"
        "CHANGELOG.md contains complete commit descriptions. CURRENT_FEATURES.md describes current capabilities and limitations.\n")
    lines = [f"{sha256(path)}  {path.name}\n" for path in sorted(output.iterdir(), key=lambda p: p.name)]
    write_utf8(output / "SHA256SUMS.txt", "".join(lines))
    validate_artifact(output, source_sha)
    summary(f"Validated Klatter {version}, source `{source_sha}`, SHA-256 `{info['sha256']}`.")


def validate_artifact(root, source_sha):
    valid_sha(source_sha)
    info = json.loads((root / "source.json").read_text(encoding="utf-8"))
    if info.get("source_sha") != source_sha or info.get("upstream") != UPSTREAM:
        raise ValueError("Artifact provenance mismatch")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+_-]{0,79}", info.get("upstream_version", "")):
        raise ValueError("Invalid upstream version metadata")
    if info.get("previous_sha"):
        valid_sha(info["previous_sha"])
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
    expected_lines = [f"{sha256(path)}  {path.name}\n" for path in sorted(entries, key=lambda p: p.name) if path.name != "SHA256SUMS.txt"]
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


def create_draft(source_sha, title, notes_path):
    """Use the creation response; a new draft may not appear in list/tag reads yet."""
    tag = release_tag(source_sha)
    request_path = notes_path.with_name("release-request.json")
    write_utf8(request_path, json.dumps({
        "tag_name": tag, "target_commitish": source_sha, "name": title,
        "body": notes_path.read_text(encoding="utf-8"), "draft": True,
    }))
    result = run("gh", "api", "--method", "POST", f"repos/{DESTINATION}/releases",
                 "--input", str(request_path))
    release = json.loads(result.stdout)
    if (type(release.get("id")) is not int or release["id"] <= 0 or
            release.get("draft") is not True or release.get("tag_name") != tag or
            release.get("target_commitish") != source_sha):
        raise ValueError("Created draft identity mismatch")
    return release


def publish(root, source_sha):
    info = validate_artifact(root, source_sha)
    tag = release_tag(source_sha)
    release = find_release(source_sha)
    if release_complete(release, source_sha):
        verify_tag(source_sha)
        summary("This source commit is already published; no assets changed.")
        return
    if release and release.get("target_commitish") != source_sha:
        raise ValueError("Draft release targets a different commit")
    verify_tag(source_sha, missing_ok=True)
    notes = (
        f"**Klatter community build {info['version']}**, upstream version **{info['upstream_version']}**, "
        "by Saqib Shaikh from "
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
    full_changes = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    if len(notes) + len(full_changes) < 100000:
        notes += "\n---\n\n" + full_changes
    else:
        notes += f"\nThe complete {info['commit_count']}-commit changelog is attached as **CHANGELOG.md**; "
        notes += "it exceeds GitHub's release-description size limit. No messages were removed from the attachment.\n"
    notes += "\nSee **CURRENT_FEATURES.md** for the complete current upstream documentation, features and known limitations.\n"
    notes_path = root.parent / "release-notes.md"
    notes_path.write_text(notes, encoding="utf-8")
    if not release:
        release = create_draft(source_sha,
            f"Klatter {info['version']} for NVDA ({source_sha[:12]})", notes_path)
    if not release or not release.get("draft") or release.get("target_commitish") != source_sha:
        raise ValueError("Expected an unpublished, source-matching draft before upload")
    release_path = f"repos/{DESTINATION}/releases/{release['id']}"
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
    parser.add_argument("action", choices=["check", "prepare", "package", "publish"])
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--source", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if os.environ.get("GITHUB_REPOSITORY") != DESTINATION:
        raise ValueError("This automation is configured only for " + DESTINATION)
    if args.action == "check":
        check(args.root.resolve())
    elif args.action == "prepare":
        prepare(args.source.resolve(), os.environ["SOURCE_SHA"], os.environ.get("PREVIOUS_SHA", ""),
                os.environ["GITHUB_RUN_NUMBER"], version_override=os.environ["COMMUNITY_VERSION"])
    elif args.action == "package":
        package(args.source.resolve(), args.output.resolve(), os.environ["SOURCE_SHA"])
    else:
        publish(args.root.resolve(), os.environ["SOURCE_SHA"])


if __name__ == "__main__":
    main()
