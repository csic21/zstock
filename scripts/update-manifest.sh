#!/usr/bin/env bash
# Generate the Zed-style static update manifest (updates/stable.json).
#
# The app polls this tiny JSON instead of the GitHub API. The release
# workflow runs this script on every `v*` tag and publishes the result
# to the repo's `main` branch, where it is served statically via
# raw.githubusercontent.
#
# Usage: update-manifest.sh <tag> <arm64-sha256> <x64-sha256> <windows-sha256> <linux-x64-sha256>
set -euo pipefail

TAG="${1:?usage: update-manifest.sh <tag> <arm64-sha256> <x64-sha256> <windows-sha256> <linux-x64-sha256>}"
SHA_ARM64="${2:?missing arm64 sha256}"
SHA_X64="${3:?missing x64 sha256}"
SHA_WIN="${4:?missing windows sha256}"
SHA_LINUX="${5:?missing linux-x64 sha256}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="${GITHUB_REPOSITORY:-csic21/zstock}"
VERSION="${TAG#v}"

# Release notes are intentionally left empty here; the app surfaces the
# release page URL instead of inline notes.
NOTES=""

mkdir -p "$ROOT/updates"
# Pure-bash heredoc: no jq or other external tools, so this works on any
# GitHub-hosted runner regardless of preinstalled software.
cat > "$ROOT/updates/stable.json" <<EOF
{
  "version": "$VERSION",
  "notes": "",
  "release_url": "https://github.com/$REPO/releases/tag/$TAG",
  "platforms": {
    "macos-arm64": {
      "url": "https://github.com/$REPO/releases/download/$TAG/zstock-macos-arm64.zip",
      "sha256": "$SHA_ARM64"
    },
    "macos-x64": {
      "url": "https://github.com/$REPO/releases/download/$TAG/zstock-macos-x64.zip",
      "sha256": "$SHA_X64"
    },
    "windows-x64": {
      "url": "https://github.com/$REPO/releases/download/$TAG/zstock-windows-x64.zip",
      "sha256": "$SHA_WIN"
    },
    "linux-x64": {
      "url": "https://github.com/$REPO/releases/download/$TAG/zstock-linux-x64.zip",
      "sha256": "$SHA_LINUX"
    }
  }
}
EOF

echo "==> wrote $ROOT/updates/stable.json"
cat "$ROOT/updates/stable.json"
