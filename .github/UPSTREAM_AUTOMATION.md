# Automatic Klatter NVDA builds

This fork checks [SaqibS/klatter](https://github.com/SaqibS/klatter)'s `main` branch at minute 23 of every hour. GitHub schedules can be delayed; this is periodic checking rather than an immediate upstream webhook. These checks do not use Codex/ChatGPT tokens. Standard GitHub-hosted runners are free for this public repository; no paid runner or personal access token is configured.

- [Latest release](https://github.com/kaveinthran/klatter/releases/latest)
- [Latest add-on download](https://github.com/kaveinthran/klatter/releases/latest/download/klatter.nvda-addon)
- [Workflow status and manual Run workflow control](https://github.com/kaveinthran/klatter/actions/workflows/upstream-nvda.yml)

## What happens

1. The check job fetches the exact latest upstream source commit. It normally merges source into this fork's `main`, preserves the fork-owned `.github` directory, and refuses unresolved source conflicts. It uses normal pushes and does not replace history.
2. A complete published release for that source commit skips the Windows build and publication. Otherwise, a standard Windows runner builds through the upstream NVDA builder and runs the Rust, NVDA driver and Python tools tests.
3. The package is checked for source identity, manifest, ZIP integrity, x64 DLL, expected files and preserved dictionary licence. A separate job validates the artifact again and publishes only after the checks pass.
4. Every source commit gets its own `upstream-<full-commit-SHA>` tag. Release files include the versioned installer, a stable `klatter.nvda-addon` alias, SHA256SUMS.txt, source.json, installation instructions, the MIT licence, CHANGELOG.md and CURRENT_FEATURES.md. Earlier releases remain available.

The upstream version may remain `0.1.0` across code changes. With the user's approval, the NVDA manifest gets an increasing numeric community version `YEAR.MONTHDAY.RUN_NUMBER`, for example `2026.1116.2`, using the UTC build date and this workflow's increasing run number. This has exactly three numeric components so NVDA store catalogs can compare every build, including several in one day. The original upstream version and source commit remain in source.json and release notes. Only NVDA packaging metadata is amended; engine/driver code is unchanged. The stable links above resolve to the latest successful published build. The existing manual release remains unchanged under its original tag.

CHANGELOG.md includes every commit subject and full message since the previous successful automated build, with links to each complete diff and the overall comparison. The first automated release includes the full upstream history. CURRENT_FEATURES.md is the entire upstream README at the exact built commit, so current capabilities and limitations are available even on the first release. Long changelogs remain complete in the attachment if they exceed GitHub's release-description limit.

## Permissions and recovery

Upstream code executes only in the build job with repository read access; checkout credentials are not persisted. The sync and publish jobs have contents write access and execute only fork-owned scripts. The publishing job treats downloaded artifacts as data. Official actions are pinned by commit SHA. No personal access token or repository secret is required.

Runs are serialized. A failed test/build leaves the previous release as the latest successful download; later hourly checks retry an unpublished source commit. A partially uploaded draft can be resumed: its original community version is recovered before rebuilding, so a later retry uses the same versioned filename. Published releases are never overwritten automatically. Unexpected package-layout changes fail for review rather than silently including new files.

GitHub can disable scheduled workflows after 60 days without repository activity. The check job records a small `.github/upstream-status.json` commit initially and no more often than every 28 days to keep this fork active. The status file records the upstream commit at that maintenance check, not every hourly check; use each run's summary for the current check.

If there is a failure, inspect the failed job in Actions, correct the automation or wait for an upstream fix, and use **Run workflow** to retry immediately. To stop the automation, use **Disable workflow** in the workflow page's menu. GitHub outage, account restrictions, revoked permissions or incompatible upstream build changes can still require maintenance.

## Installation boundary

The automation publishes GitHub downloads. It does not install or update NVDA on your computer. Automated driver tests use NVDA stand-ins; actual speech and live NVDA installation still require user acceptance.

## References

- [GitHub Actions scheduling and inactivity rules](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule)
- [Public-repository runner billing](https://docs.github.com/en/actions/concepts/billing-and-usage)
- [Job token permissions](https://docs.github.com/en/actions/tutorials/authenticate-with-github_token)
