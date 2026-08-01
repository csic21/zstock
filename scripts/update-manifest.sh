#!/usr/bin/env bash
# Generate the Zed-style static update manifest (updates/stable.json).
#
# The app polls this tiny JSON instead of the GitHub API. The release
# workflow runs this script on every `v*` tag and publishes the result
# to the repo's `main` branch, where it is served statically via
# raw.githubusercontent.
#
# Usage: update-manifest.sh <tag> <arm64-sha256> <x64-sha256> <windows-sha256>
set -euo pipefail

TAG="${1:?usage: update-manifest.sh <tag> <arm64-sha256> <x64-sha256> <windows-sha256>}"
SHA_ARM64="${2:?missing arm64 sha256}"
SHA_X64="${3:?missing x64 sha256}"
SHA_WIN="${4:?missing windows sha256}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="${GITHUB_REPOSITORY:-csic21/zstock}"
VERSION="${TAG#v}"

# Release notes are intentionally left empty here; the app surfaces the
# release page URL instead of inline notes.
NOTES=""

mkdir -p "$ROOT/updates"
jq -n \
  --arg version "$VERSION" \
  --arg notes "$NOTES" \
  --arg release_url "https://github.com/$REPO/releases/tag/$TAG" \
  --arg url_arm64 "https://github.com/$REPO/releases/download/$TAG/stock-analysis-macos-arm64.zip" \
  --arg sha_arm64 "$SHA_ARM64" \
  --arg url_x64 "https://github.com/$REPO/releases/download/$TAG/stock-analysis-macos-x64.zip" \
  --arg sha_x64 "$SHA_X64" \
  --arg url_win "https://github.com/$REPO/releases/download/$TAG/stock-analysis-windows-x64.zip" \
  --arg sha_win "$SHA_WIN" \
  '{version:$version, notes:$notes, release_url:$release_url, platforms:{"macos-arm64":{url:$url_arm64,sha256:$sha_arm64},"macos-x64":{url:$url_x64,sha256:$sha_x64},"windows-x64":{url:$url_win,sha256:$sha_win}}}' \
  > "$ROOT/updates/stable.json"

echo "==> wrote $ROOT/updates/stable.json"
cat "$ROOT/updates/stable.json"
