# Contributing

This GitHub repository is a mirror. Please open merge requests and file issues through the GitLab project:

**https://gitlab.com/tnoff-projects/harpocrates**

## Release flow

Releases are split across GitLab and GitHub for now:

- CI, dependency updates (Renovate), version bumps, and tagging all happen on GitLab.
- When `tag-build` creates a new `vX.Y.Z` tag on GitLab, the tag is mirrored to GitHub.
- The GitHub `release.yml` workflow triggers on the tag push and builds the cross-platform Tauri installers (Linux, macOS arm64/x86_64, Windows), then attaches them to a GitHub Release.

GitHub is used for the release builds because GitLab's self-hosted runners are arm64 Linux only and can't produce macOS or Windows installers natively. The intent is to migrate the release flow to GitLab once cross-platform build runners are available there too.
