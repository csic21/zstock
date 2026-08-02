#!/usr/bin/env bash
# Build a macOS .app bundle with the S monogram icon.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="ZStock"
BUNDLE_ID="com.karl.stock-analysis"
BINARY_NAME="stock"
DIST="${ROOT}/dist"
APP_DIR="${DIST}/${APP_NAME}.app"
CONTENTS="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS}/MacOS"
RES_DIR="${CONTENTS}/Resources"
VERSION="$(sed -nE 's/^version = "([^"]+)".*/\1/p' Cargo.toml | head -1)"
test -n "$VERSION" || { echo "error: could not read version from Cargo.toml" >&2; exit 1; }

if [ "${SKIP_CARGO_BUILD:-0}" != "1" ]; then
  echo "==> cargo build --release"
  cargo build --release
fi

echo "==> assemble ${APP_NAME}.app (v${VERSION})"
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RES_DIR"

cp "target/release/${BINARY_NAME}" "${MACOS_DIR}/${BINARY_NAME}"
chmod +x "${MACOS_DIR}/${BINARY_NAME}"

cp "assets/macos/Info.plist" "${CONTENTS}/Info.plist"
# Keep CFBundle*Version in sync with Cargo.toml for local packages and CI
# (release workflow also rewrites assets/macos/Info.plist on tags).
if command -v plutil >/dev/null 2>&1; then
  plutil -replace CFBundleShortVersionString -string "$VERSION" "${CONTENTS}/Info.plist"
  plutil -replace CFBundleVersion -string "$VERSION" "${CONTENTS}/Info.plist"
  plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "${CONTENTS}/Info.plist"
else
  # Best-effort fallback when plutil is unavailable (should not happen on macOS).
  sed -i.bak \
    -e "s#<string>0.0.1</string>#<string>${VERSION}</string>#g" \
    "${CONTENTS}/Info.plist" && rm -f "${CONTENTS}/Info.plist.bak"
fi
cp "assets/logo/AppIcon.icns" "${RES_DIR}/AppIcon.icns"

# Ad-hoc sign so Gatekeeper is a bit less noisy on local machines
if command -v codesign >/dev/null 2>&1; then
  echo "==> codesign --force -s - (ad-hoc)"
  codesign --force -s - "${APP_DIR}" 2>/dev/null || true
fi

echo "==> done: ${APP_DIR}"
echo "    open \"${APP_DIR}\""
ls -lh "${MACOS_DIR}/${BINARY_NAME}" "${RES_DIR}/AppIcon.icns"
