"""Offline regression tests for publication boundaries and source-sync recovery."""
from datetime import datetime, timedelta, timezone
import json
import io
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import auto_release as automation

SHA = "a" * 40


def write_package(path, extra=False):
    dll = bytearray(256)
    dll[:2] = b"MZ"
    struct.pack_into("<I", dll, 0x3C, 128)
    dll[128:132] = b"PE\0\0"
    struct.pack_into("<H", dll, 132, 0x8664)
    struct.pack_into("<H", dll, 150, 0x2000)
    entries = {
        "manifest.ini": b'name = klatter\nversion = 0.1.0\nminimumNVDAVersion = 2026.1\nlastTestedNVDAVersion = 2026.2\n',
        "synthDrivers/klatter/__init__.py": b"# test driver\n",
        "synthDrivers/klatter/klatter.dll": dll,
        "synthDrivers/klatter/cmudict-LICENSE.txt": b"dictionary licence\n",
    }
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in entries.items():
            archive.writestr(name, content)
        if extra:
            archive.writestr("../private.txt", "unexpected")
    return entries


class ReleaseTests(unittest.TestCase):
    def complete_release(self):
        return {"draft": False, "target_commitish": SHA,
                "assets": [{"name": n, "state": "uploaded", "size": 1, "digest": "sha256:" + "a" * 64} for n in
                           automation.FIXED_ASSETS | {"klatter-0.1.0.nvda-addon"}]}

    def test_missing_and_interrupted_draft_are_retryable(self):
        self.assertFalse(automation.release_complete(None, SHA))
        self.assertFalse(automation.release_complete({"draft": True}, SHA))

    def test_draft_is_recovered_when_github_has_not_created_its_tag(self):
        draft = {"id": 42, "draft": True, "tag_name": automation.release_tag(SHA), "target_commitish": SHA}
        with patch.object(automation, "api", side_effect=[None, [draft]]):
            self.assertEqual(automation.find_release(SHA), draft)

    def test_complete_commit_skips_but_wrong_commit_is_rejected(self):
        release = self.complete_release()
        self.assertTrue(automation.release_complete(release, SHA))
        with self.assertRaisesRegex(ValueError, "different commit"):
            automation.release_complete(release, "b" * 40)

    def test_incomplete_public_release_is_not_overwritten(self):
        release = self.complete_release()
        release["assets"] = []
        with self.assertRaisesRegex(ValueError, "incomplete"):
            automation.release_complete(release, SHA)

    def test_unsafe_source_ref_is_rejected(self):
        with self.assertRaises(ValueError):
            automation.release_tag("main; injected")

    def test_published_release_cannot_silently_include_extra_assets(self):
        release = self.complete_release()
        release["assets"].append({"name": "private.log", "state": "uploaded", "size": 1,
                                  "digest": "sha256:" + "a" * 64})
        with self.assertRaisesRegex(ValueError, "incomplete"):
            automation.release_complete(release, SHA)

    def test_release_tag_must_resolve_to_the_actual_source(self):
        with patch.object(automation, "api", return_value={"object": {"type": "commit", "sha": "b" * 40}}):
            with self.assertRaisesRegex(ValueError, "tag"):
                automation.verify_tag(SHA)

    def test_change_baseline_ignores_newer_manual_releases_and_unpublished_drafts(self):
        releases = [
            {"tag_name": "upstream-old", "target_commitish": SHA, "published_at": "2026-11-15T12:00:00Z"},
            {"tag_name": "manual", "target_commitish": "b" * 40, "published_at": "2026-11-16T12:00:00Z"},
            {"tag_name": "upstream-draft", "target_commitish": "c" * 40, "draft": True,
             "published_at": "2026-11-17T12:00:00Z"},
        ]
        self.assertEqual(automation.previous_source(releases), SHA)
        self.assertEqual(automation.previous_source([]), "")


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        self.product = automation.DESTINATION.rsplit("/", 1)[1].capitalize()
        addon = self.product.lower() + ".nvda-addon"
        self.intro = (f"**{self.product} community build 2026.1118.8**, upstream source.\n\n"
                      f"Download **{addon}** below. Requires NVDA 2026.1.\n\n"
                      "Tests passed; live installation has not been checked.\n\n"
                      "Build run and checksum.\n\nLicence information.\n")
        self.changes = ("# Complete changelog\n\nChanges since the previous successful source build.\n\n"
                        f"## Older fix\n\n[{'b' * 40}](https://github.com/{automation.UPSTREAM}/commit/{'b' * 40}) - 2026-11-16T00:00:00Z\n\n> Full old details.\n"
                        f"## Newest [voice] fix\n\n[{SHA}](https://github.com/{automation.UPSTREAM}/commit/{SHA}) - 2026-11-17T00:00:00Z\n\n> Full new details.\n")

    def test_legacy_changelog_is_presented_newest_first_with_downloads(self):
        notes = automation.format_release_notes(self.intro, self.changes, SHA)
        self.assertLess(notes.index("Newest"), notes.index("Older fix"))
        self.assertLess(notes.index("Changes in this release"), notes.index("Build verification"))
        self.assertIn("Newest \\[voice\\] fix", notes)
        self.assertIn("/CHANGELOG.md)", notes)
        self.assertNotIn("> Full new details", notes)
        self.assertNotIn("# Complete changelog", notes)

    def test_new_attachment_keeps_messages_and_newest_first_heading_order(self):
        raw = (SHA + "\0" + "2026-11-17T00:00:00Z\0Newest fix\n\nAll new details.\0" +
               "b" * 40 + "\0" + "2026-11-16T00:00:00Z\0Older fix\n\nAll older details.\0")
        with patch.object(automation, "run", return_value=subprocess.CompletedProcess([], 0, raw, "")):
            changes, commits = automation.changelog(Path.cwd(), SHA, "", "2026.1118.8", "1.0")
        self.assertEqual([item["sha"] for item in commits], [SHA, "b" * 40])
        self.assertIn("> All new details.", changes)
        self.assertIn("> All older details.", changes)
        self.assertIn("## Changes at a glance", changes)
        self.assertIn("### Newest fix", changes)
        notes = automation.format_release_notes(self.intro, changes, SHA)
        self.assertLess(notes.index("Newest fix"), notes.index("Older fix"))

    def test_history_replacement_is_not_described_as_all_new(self):
        notes = automation.format_release_notes(self.intro,
            self.changes.replace("Changes since", "Upstream history diverged; changes since"), SHA)
        self.assertIn("## Recent upstream work", notes)
        self.assertIn("must not be read as all-new features", notes)
        self.assertNotIn("## Changes in this release", notes)
        notes = automation.format_release_notes(self.intro, "First automated build:\n" + self.changes, SHA)
        self.assertIn("## Included upstream work", notes)

    def test_description_refresh_is_idempotent_and_does_not_change_assets(self):
        release = ReleaseTests().complete_release()
        content = self.changes.encode()
        release.update(id=42, tag_name=automation.release_tag(SHA), body=self.intro + "\n---\n" + self.changes)
        for asset in release["assets"]:
            if asset["name"] == "CHANGELOG.md":
                asset.update(size=len(content), digest="sha256:" + automation.hashlib.sha256(content).hexdigest())
        notes = automation.format_release_notes(self.intro, self.changes, SHA)
        updated = release | {"body": notes}
        with patch.object(automation.urllib.request, "urlopen", side_effect=lambda *a, **k: io.BytesIO(content)), \
                patch.object(automation, "run", return_value=
                    subprocess.CompletedProcess([], 0, json.dumps(updated), "")) as run, \
                patch.object(automation, "summary"):
            automation.refresh_release_notes(release, SHA)
            automation.refresh_release_notes(updated, SHA)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[1:4], ("api", "--method", "PATCH"))

    def test_tampered_published_changelog_is_rejected_before_edit(self):
        release = ReleaseTests().complete_release()
        release.update(id=42, body=self.intro + "\n---\n" + self.changes)
        with patch.object(automation.urllib.request, "urlopen", return_value=io.BytesIO(b"bad")), \
                patch.object(automation, "run") as run:
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                automation.refresh_release_notes(release, SHA)
            run.assert_not_called()


class DraftCreationTests(unittest.TestCase):
    def test_creation_response_is_used_without_a_second_lookup(self):
        draft = {"id": 42, "draft": True, "tag_name": automation.release_tag(SHA),
                 "target_commitish": SHA}
        with tempfile.TemporaryDirectory() as temporary:
            notes = Path(temporary) / "release-notes.md"
            notes.write_text("Complete release notes.\n", encoding="utf-8")
            with patch.object(automation, "run", return_value=
                    subprocess.CompletedProcess([], 0, json.dumps(draft), "")) as run, \
                    patch.object(automation, "find_release", side_effect=AssertionError("Stale lookup")):
                self.assertEqual(automation.create_draft(SHA, "Community build", notes), draft)
            request = json.loads(notes.with_name("release-request.json").read_text())
            self.assertEqual(request, {"tag_name": automation.release_tag(SHA),
                "target_commitish": SHA, "name": "Community build",
                "body": "Complete release notes.\n", "draft": True})
            self.assertEqual(run.call_args.args[:4], ("gh", "api", "--method", "POST"))

    def test_wrong_identity_or_published_response_is_rejected(self):
        draft = {"id": 42, "draft": True, "tag_name": automation.release_tag(SHA),
                 "target_commitish": SHA}
        with tempfile.TemporaryDirectory() as temporary:
            notes = Path(temporary) / "release-notes.md"
            notes.write_text("Notes", encoding="utf-8")
            for changes in ({"id": None}, {"id": True}, {"id": -1}, {"draft": False},
                            {"tag_name": "different"}, {"target_commitish": "b" * 40}):
                with self.subTest(changes=changes), patch.object(automation, "run", return_value=
                        subprocess.CompletedProcess([], 0, json.dumps(draft | changes), "")):
                    with self.assertRaisesRegex(ValueError, "identity mismatch"):
                        automation.create_draft(SHA, "Community build", notes)


class CheckTests(unittest.TestCase):
    def test_advertised_main_is_used_instead_of_commit_api(self):
        result = subprocess.CompletedProcess([], 0, SHA + "\trefs/heads/main\n", "")
        with patch.object(automation, "run", return_value=result) as command:
            self.assertEqual(automation.upstream_head(Path.cwd()), SHA)
        self.assertEqual(command.call_args.args[1:4], ("ls-remote", "--exit-code", "--refs"))

    def test_bad_or_ambiguous_advertisement_fails(self):
        for output in ("", SHA + " refs/heads/other", "unsafe refs/heads/main",
                       (SHA + " refs/heads/main\n") * 2):
            with self.subTest(output=output), patch.object(automation, "run", return_value=
                    subprocess.CompletedProcess([], 0, output, "")):
                with self.assertRaises(ValueError):
                    automation.upstream_head(Path.cwd())

    def test_new_source_builds_even_when_old_source_has_a_release(self):
        self.check_outputs(None, "true")

    def test_complete_source_skips_and_checks_tag(self):
        self.check_outputs(ReleaseTests().complete_release(), "false")

    def test_unpublished_draft_retries(self):
        draft = {"draft": True, "target_commitish": SHA}
        self.check_outputs(draft, "true")

    def check_outputs(self, release, needed):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "outputs"
            old = "b" * 40
            releases = [{"tag_name": "upstream-" + old, "target_commitish": old,
                         "published_at": "2026-11-16T00:00:00Z"}]
            with patch.object(automation, "upstream_head", return_value=SHA), \
                    patch.object(automation, "sync_source") as sync, \
                    patch.object(automation, "find_release", return_value=release) as find, \
                    patch.object(automation, "api", return_value=releases) as api, \
                    patch.object(automation, "verify_tag") as verify, \
                    patch.object(automation, "refresh_release_notes"), \
                    patch.object(automation, "community_version", return_value="2026.1118.8"), \
                    patch.object(automation, "summary"), \
                    patch.dict(os.environ, {"GITHUB_OUTPUT": str(output), "GITHUB_RUN_NUMBER": "8"}):
                automation.check(root)
            values = dict(line.split("=", 1) for line in output.read_text().splitlines())
            self.assertEqual(values["sha"], SHA)
            self.assertEqual(values["build_needed"], needed)
            self.assertEqual(values["previous_sha"], old)
            self.assertEqual(values["community_version"], "2026.1118.8" if needed == "true" else "")
            sync.assert_called_once_with(root, SHA)
            find.assert_called_once_with(SHA)
            self.assertEqual(verify.call_count, needed == "false")
            self.assertNotIn("commits/main", api.call_args.args[0])


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        (self.source / "target").mkdir(parents=True)
        entries = write_package(self.source / "target/klatter-0.1.0.nvda-addon")
        for name, content in entries.items():
            path = self.source / "nvda/addon" / name
            if name.endswith("cmudict-LICENSE.txt"):
                path = self.source / "data/cmudict/LICENSE.txt"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        (self.source / "LICENSE.md").write_text("MIT licence\n", encoding="utf-8")
        (self.source / "readme.md").write_text("# Current features\n\nEnglish speech.\n", encoding="utf-8")
        (self.source / ".community-build.json").write_text(json.dumps({"source_sha": SHA, "version": "0.1.0",
                "upstream_version": "0.1.0", "previous_sha": ""}), encoding="utf-8")
        self.output = self.root / "release"
        result = subprocess.CompletedProcess([], 0, SHA + "\n", "")
        with patch.object(automation, "run", return_value=result), patch.object(automation, "summary"), \
                patch.object(automation, "changelog", return_value=("# Full change details\n", [{"sha": SHA}])):
            automation.package(self.source, self.output, SHA)

    def test_valid_artifact_and_alias_round_trip(self):
        info = automation.validate_artifact(self.output, SHA)
        self.assertEqual(info["version"], "0.1.0")
        self.assertEqual(len(list(self.output.iterdir())), 8)

    def test_checksums_use_the_same_order_and_lf_on_windows_and_linux(self):
        path = self.output / "SHA256SUMS.txt"
        names = [line.split("  ", 1)[1] for line in path.read_text(encoding="utf-8").splitlines()]
        self.assertEqual(names, sorted(names))
        self.assertNotIn(b"\r\n", path.read_bytes())

    def test_tampered_alias_is_rejected(self):
        (self.output / "klatter.nvda-addon").write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "alias mismatch"):
            automation.validate_artifact(self.output, SHA)

    def test_provenance_cannot_override_checked_source(self):
        with self.assertRaisesRegex(ValueError, "provenance"):
            automation.validate_artifact(self.output, "b" * 40)

    def test_unexpected_artifact_file_is_rejected(self):
        (self.output / "secret.log").write_text("not for publication", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Unexpected release files"):
            automation.validate_artifact(self.output, SHA)

    def test_archive_traversal_entry_is_rejected(self):
        path = self.root / "bad.nvda-addon"
        write_package(path, extra=True)
        with self.assertRaisesRegex(ValueError, "payload"):
            automation.inspect_package(path)

    def test_compatibility_metadata_must_match_the_actual_addon(self):
        path = self.output / "source.json"
        info = json.loads(path.read_text(encoding="utf-8"))
        info["minimum_nvda"] = "2020.1"
        path.write_text(json.dumps(info), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "compatibility"):
            automation.validate_artifact(self.output, SHA)


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "upstream"
        self.fork = self.root / "fork"
        self.bare = self.root / "origin.git"
        self.real_run = automation.run
        self.real_run("git", "init", "-b", "main", str(self.source))
        for name, value in [("user.name", "Test"), ("user.email", "test@example.invalid")]:
            self.real_run("git", "config", name, value, cwd=self.source)
        (self.source / "engine.txt").write_text("original\n", encoding="utf-8")
        (self.source / ".github").mkdir()
        (self.source / ".github/shared.txt").write_text("upstream original\n", encoding="utf-8")
        (self.source / ".github/removed.txt").write_text("keep this file\n", encoding="utf-8")
        self.commit(self.source, "Initial source")
        self.real_run("git", "clone", "--bare", str(self.source), str(self.bare))
        self.real_run("git", "clone", str(self.bare), str(self.fork))
        for name, value in [("user.name", "Test"), ("user.email", "test@example.invalid")]:
            self.real_run("git", "config", name, value, cwd=self.fork)
        (self.fork / ".github/workflows").mkdir(parents=True)
        (self.fork / ".github/workflows/trusted.yml").write_text("trusted workflow\n", encoding="utf-8")
        (self.fork / ".github/shared.txt").write_text("fork-owned configuration\n", encoding="utf-8")
        self.commit(self.fork, "Trusted fork automation")
        self.real_run("git", "push", "origin", "main", cwd=self.fork)
        self.now = datetime(2026, 11, 16, tzinfo=timezone.utc)

    def commit(self, root, message):
        self.real_run("git", "add", ".", cwd=root)
        self.real_run("git", "commit", "-m", message, cwd=root)
        return self.real_run("git", "rev-parse", "HEAD", cwd=root).stdout.strip()

    def local_run(self, *args, **kwargs):
        if args[:3] == ("git", "fetch", "--no-tags"):
            args = (*args[:3], str(self.source), args[-1])
        return self.real_run(*args, **kwargs)

    def sync(self, source_sha, now=None):
        with patch.object(automation, "run", side_effect=self.local_run), patch.dict(os.environ, {"GH_TOKEN": "test-only"}):
            automation.sync_source(self.fork, source_sha, now or self.now)

    def test_merge_updates_engine_but_blocks_new_upstream_workflow(self):
        (self.source / "engine.txt").write_text("updated source\n", encoding="utf-8")
        (self.source / ".github/workflows").mkdir(parents=True)
        (self.source / ".github/workflows/untrusted.yml").write_text("upstream workflow\n", encoding="utf-8")
        source_sha = self.commit(self.source, "Source with upstream workflow")
        self.sync(source_sha)
        self.assertEqual((self.fork / "engine.txt").read_text(encoding="utf-8"), "updated source\n")
        self.assertEqual((self.fork / ".github/workflows/trusted.yml").read_text(encoding="utf-8"), "trusted workflow\n")
        self.assertFalse((self.fork / ".github/workflows/untrusted.yml").exists())
        self.real_run("git", "merge-base", "--is-ancestor", source_sha, "HEAD", cwd=self.fork)

    def test_merge_preserves_modified_and_deleted_automation_files(self):
        (self.source / ".github/shared.txt").write_text("untrusted edit\n", encoding="utf-8")
        (self.source / ".github/removed.txt").unlink()
        source_sha = self.commit(self.source, "Modify and delete upstream configuration")
        self.sync(source_sha)
        self.assertEqual((self.fork / ".github/shared.txt").read_text(encoding="utf-8"), "fork-owned configuration\n")
        self.assertEqual((self.fork / ".github/removed.txt").read_text(encoding="utf-8"), "keep this file\n")

    def test_source_conflict_preserves_remote_and_aborts_merge(self):
        (self.fork / "engine.txt").write_text("local change\n", encoding="utf-8")
        self.commit(self.fork, "Local source change")
        before = self.real_run("git", "rev-parse", "main", cwd=self.bare).stdout
        (self.source / "engine.txt").write_text("different upstream change\n", encoding="utf-8")
        source_sha = self.commit(self.source, "Conflicting source change")
        with self.assertRaisesRegex(RuntimeError, "conflicts"):
            self.sync(source_sha)
        self.assertEqual(self.real_run("git", "rev-parse", "main", cwd=self.bare).stdout, before)
        self.assertEqual((self.fork / "engine.txt").read_text(encoding="utf-8"), "local change\n")

    def test_no_change_has_no_hourly_commit_but_28_days_keeps_schedule_active(self):
        source_sha = self.real_run("git", "rev-parse", "HEAD", cwd=self.source).stdout.strip()
        self.sync(source_sha)
        first = self.real_run("git", "rev-parse", "HEAD", cwd=self.fork).stdout
        self.sync(source_sha, self.now + timedelta(hours=1))
        self.assertEqual(self.real_run("git", "rev-parse", "HEAD", cwd=self.fork).stdout, first)
        self.sync(source_sha, self.now + timedelta(days=28))
        self.assertNotEqual(self.real_run("git", "rev-parse", "HEAD", cwd=self.fork).stdout, first)

    def test_replaced_history_imports_exact_source_without_losing_automation(self):
        old = self.real_run("git", "rev-parse", "HEAD", cwd=self.source).stdout.strip()
        (self.fork / "obsolete.txt").write_text("old source\n")
        self.commit(self.fork, "Old source only file")
        self.real_run("git", "checkout", "--orphan", "replacement", cwd=self.source)
        self.real_run("git", "rm", "-rf", ".", cwd=self.source)
        (self.source / "engine.txt").write_text("replacement source\n")
        (self.source / ".github/workflows").mkdir(parents=True)
        (self.source / ".github/workflows/untrusted.yml").write_text("untrusted\n")
        current = self.commit(self.source, "Replacement history")
        self.sync(current)
        self.assertEqual((self.fork / "engine.txt").read_text(), "replacement source\n")
        self.assertFalse((self.fork / "obsolete.txt").exists())
        self.assertFalse((self.fork / ".github/workflows/untrusted.yml").exists())
        self.assertEqual((self.fork / ".github/workflows/trusted.yml").read_text(), "trusted workflow\n")
        self.real_run("git", "merge-base", "--is-ancestor", old, "HEAD", cwd=self.fork)
        self.real_run("git", "merge-base", "--is-ancestor", current, "HEAD", cwd=self.fork)
        changes, commits = automation.changelog(self.source, current, "f" * 40, "2026.1118.8", "0.1.0")
        self.assertIn("history diverged", changes)
        self.assertEqual([commit["sha"] for commit in commits], [current])
        first = self.real_run("git", "rev-parse", "HEAD", cwd=self.fork).stdout
        self.sync(current)
        self.assertEqual(self.real_run("git", "rev-parse", "HEAD", cwd=self.fork).stdout, first)

    def test_full_first_history_and_incremental_history_preserve_complete_messages(self):
        previous = self.real_run("git", "rev-parse", "HEAD", cwd=self.source).stdout.strip()
        (self.source / "engine.txt").write_text("new feature\n", encoding="utf-8")
        current = self.commit(self.source, "New feature\n\nComplete first detail.\nComplete second detail.")
        first, all_commits = automation.changelog(self.source, current, "", "2026.1116.1", "0.1.0")
        self.assertIn("Initial source", first)
        self.assertIn("Complete second detail.", first)
        self.assertEqual(len(all_commits), 2)
        incremental, commits = automation.changelog(self.source, current, previous, "2026.1116.2", "0.1.0")
        self.assertNotIn("## Initial source", incremental)
        self.assertIn("Complete first detail.", incremental)
        self.assertEqual([commit["sha"] for commit in commits], [current])


class VersionTests(unittest.TestCase):
    def test_partial_draft_retry_keeps_the_same_versioned_asset_name(self):
        draft = {"draft": True, "target_commitish": SHA, "name": f"Klatter 2026.1116.2 for NVDA ({SHA[:12]})",
                 "assets": [{"name": "klatter-2026.1116.2.nvda-addon"}]}
        recovered = automation.community_version(SHA, draft, "19", datetime(2026, 11, 17, tzinfo=timezone.utc))
        self.assertEqual(recovered, "2026.1116.2")
        self.assertEqual(f"klatter-{recovered}.nvda-addon", draft["assets"][0]["name"])

    def test_community_version_increases_and_preserves_original_upstream_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            manifest = source / "nvda/addon/manifest.ini"
            manifest.parent.mkdir(parents=True)
            original = 'name = klatter\nsummary = "Klatter speech synthesizer"\nversion = 0.1.0\n'
            result = subprocess.CompletedProcess([], 0, SHA + "\n", "")
            versions = []
            for run_number, date in [("2", datetime(2026, 11, 16, tzinfo=timezone.utc)),
                                     ("9", datetime(2026, 11, 16, tzinfo=timezone.utc)),
                                     ("10", datetime(2026, 11, 17, tzinfo=timezone.utc))]:
                manifest.write_text(original, encoding="utf-8")
                with patch.object(automation, "run", return_value=result), patch.object(automation, "summary"):
                    automation.prepare(source, SHA, "", run_number, date)
                context = json.loads((source / ".community-build.json").read_text(encoding="utf-8"))
                self.assertEqual(context["upstream_version"], "0.1.0")
                self.assertIn("community build", manifest.read_text(encoding="utf-8"))
                versions.append(tuple(map(int, context["version"].split("."))))
            self.assertEqual(versions, sorted(set(versions)))


if __name__ == "__main__":
    unittest.main()
