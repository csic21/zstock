#!/usr/bin/env bash
# Build a macOS installer package (.pkg) that installs ZStock into /Applications
# without any drag-and-drop step.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="ZStock"
BUNDLE_ID="com.karl.stock-analysis"
ARCH_LABEL="${1:-}"
if [ -z "$ARCH_LABEL" ]; then
  case "$(uname -m)" in
    arm64) ARCH_LABEL="arm64" ;;
    x86_64) ARCH_LABEL="x64" ;;
    *) ARCH_LABEL="$(uname -m)" ;;
  esac
fi
VERSION="$(sed -nE 's/^version = "([^"]+)".*/\1/p' Cargo.toml | head -1)"
test -n "$VERSION" || { echo "::error::could not read version from Cargo.toml" >&2; exit 1; }

DIST="${ROOT}/dist"
PAYLOAD="${DIST}/pkg-payload"
COMPONENT="${DIST}/zstock-component.pkg"
OUT="${DIST}/zstock-macos-${ARCH_LABEL}.pkg"

# 1. Build / refresh the .app bundle.
SKIP_CARGO_BUILD="${SKIP_CARGO_BUILD:-0}" ./scripts/package-macos.sh

# 2. Assemble a clean payload so only ZStock.app is packaged.
rm -rf "$PAYLOAD"
mkdir -p "$PAYLOAD"
cp -R "${DIST}/${APP_NAME}.app" "${PAYLOAD}/"

# 3. Build the component package (preinstall removes previous installs).
rm -f "$COMPONENT"
pkgbuild \
  --root "$PAYLOAD" \
  --identifier "$BUNDLE_ID" \
  --version "$VERSION" \
  --install-location /Applications \
  --scripts "${ROOT}/scripts/macos-installer" \
  "$COMPONENT"

rm -rf "$PAYLOAD"

# 4. Wrap with a friendly zh-CN installer UI.
rm -f "$OUT"
DIST_TMP="${DIST}/.distribution.xml"
sed "s/__VERSION__/${VERSION}/" "${ROOT}/assets/macos/installer/distribution.xml" > "$DIST_TMP"
productbuild \
  --distribution "$DIST_TMP" \
  --package-path "$DIST" \
  --resources "${ROOT}/assets/macos/installer" \
  "$OUT"

rm -f "$COMPONENT"
rm -f "$DIST_TMP"

echo "==> done: ${OUT}"
echo "    open \"${OUT}\""
