#!/usr/bin/env bash
# Build a macOS DMG with the standard drag-to-Applications layout.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="ZStock"
ARCH_LABEL="${1:-}"
if [ -z "$ARCH_LABEL" ]; then
  case "$(uname -m)" in
    arm64) ARCH_LABEL="arm64" ;;
    x86_64) ARCH_LABEL="x64" ;;
    *) ARCH_LABEL="$(uname -m)" ;;
  esac
fi

DIST="${ROOT}/dist"
OUT="${DIST}/stock-analysis-macos-${ARCH_LABEL}.dmg"
STAGE="${DIST}/dmg-staging"
RW_IMG="${DIST}/.zstock-raw.dmg"

# 1. Build / refresh the .app bundle.
SKIP_CARGO_BUILD="${SKIP_CARGO_BUILD:-0}" ./scripts/package-macos.sh

# 2. Stage the classic drag-install layout: app + Applications shortcut.
rm -rf "$STAGE" "$RW_IMG"
mkdir -p "$STAGE"
cp -R "${DIST}/${APP_NAME}.app" "${STAGE}/${APP_NAME}.app"
ln -s /Applications "${STAGE}/Applications"

# 3. Create a writable image, then try to lay out icons nicely (best effort).
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDRW "$RW_IMG" >/dev/null
MOUNT_POINT="$(hdiutil attach -readwrite -noverify -noautoopen "$RW_IMG" 2>/dev/null \
  | awk -F '\t' '/\/Volumes\// {print $NF; exit}')"
if [ -n "$MOUNT_POINT" ]; then
  # Arrange icons first (Finder deletes .VolumeIcon.icns when it opens a
  # volume). Guard against hangs on headless CI runners (30s max).
  if command -v perl >/dev/null 2>&1; then
    perl -e 'alarm shift; exec @ARGV' 30 osascript >/dev/null 2>&1 <<'APPLESCRIPT' || true
tell application "Finder"
  tell disk "ZStock"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {0, 0, 620, 420}
    set viewOptions to the icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 128
    set text size of viewOptions to 14
    set position of item "ZStock.app" of container window to {170, 205}
    set position of item "Applications" of container window to {450, 205}
    update without registering applications
    close
  end tell
end tell
APPLESCRIPT
  fi
  # Volume icon: copy straight onto the mounted image and mark it custom.
  cp "assets/logo/AppIcon.icns" "${MOUNT_POINT}/.VolumeIcon.icns"
  if command -v SetFile >/dev/null 2>&1; then
    SetFile -a C "${MOUNT_POINT}/.VolumeIcon.icns" || true
  fi
  sync
  hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 \
    || hdiutil detach -force "$MOUNT_POINT" >/dev/null 2>&1 || true
fi

# 4. Compress to the final DMG.
rm -f "$OUT"
hdiutil convert "$RW_IMG" -format UDZO -o "$OUT" >/dev/null
rm -f "$RW_IMG"
rm -rf "$STAGE"

echo "==> done: ${OUT}"
echo "    open \"${OUT}\""
