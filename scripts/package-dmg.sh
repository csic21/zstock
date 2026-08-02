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
OUT="${DIST}/zstock-macos-${ARCH_LABEL}.dmg"
# Arch-specific staging so arm64 + x64 packaging in one CI job cannot collide.
STAGE="${DIST}/dmg-staging-${ARCH_LABEL}"
RW_IMG="${DIST}/.zstock-raw-${ARCH_LABEL}.dmg"
# Final volume name shown to users. Must stay "ZStock" for the Finder layout
# script below; we carefully detach leftovers before/after each run.
VOL_NAME="ZStock"

detach_volume() {
  local mp="$1"
  [ -n "$mp" ] || return 0
  if [ -e "$mp" ] || mount | grep -Fq "$mp"; then
    hdiutil detach "$mp" >/dev/null 2>&1 \
      || hdiutil detach -force "$mp" >/dev/null 2>&1 \
      || diskutil unmount force "$mp" >/dev/null 2>&1 \
      || true
  fi
}

# Clear any leftover ZStock volume from a previous arch package in this job.
detach_volume "/Volumes/${VOL_NAME}"
# Also detach anything still attached to our temp image if present.
if [ -f "$RW_IMG" ]; then
  while IFS= read -r mp; do
    [ -n "$mp" ] || continue
    detach_volume "$mp"
  done < <(hdiutil info 2>/dev/null | awk -v img="$RW_IMG" '
    $0 ~ img { hit=1 }
    hit && /\/Volumes\// { print $NF; hit=0 }
  ')
fi

# 1. Build / refresh the .app bundle.
SKIP_CARGO_BUILD="${SKIP_CARGO_BUILD:-0}" ./scripts/package-macos.sh

# 2. Stage the classic drag-install layout: app + Applications shortcut.
rm -rf "$STAGE" "$RW_IMG"
mkdir -p "$STAGE"
cp -R "${DIST}/${APP_NAME}.app" "${STAGE}/${APP_NAME}.app"
ln -s /Applications "${STAGE}/Applications"

# 3. Create a writable image, then try to lay out icons nicely (best effort).
# Retry: CI runners occasionally hit "Resource busy" if a prior volume is slow
# to release after detach.
create_ok=0
for attempt in 1 2 3; do
  detach_volume "/Volumes/${VOL_NAME}"
  if hdiutil create -volname "$VOL_NAME" -srcfolder "$STAGE" -ov -format UDRW "$RW_IMG" >/dev/null 2> "${DIST}/.hdiutil-create-${ARCH_LABEL}.err"; then
    create_ok=1
    break
  fi
  echo "==> hdiutil create attempt ${attempt} failed:" >&2
  cat "${DIST}/.hdiutil-create-${ARCH_LABEL}.err" >&2 || true
  sleep 2
done
rm -f "${DIST}/.hdiutil-create-${ARCH_LABEL}.err"
if [ "$create_ok" -ne 1 ]; then
  echo "error: hdiutil create failed for ${ARCH_LABEL} after retries" >&2
  exit 1
fi

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
  detach_volume "$MOUNT_POINT"
  # Brief pause so macOS fully releases the image before convert.
  sleep 1
fi

# 4. Compress to the final DMG.
rm -f "$OUT"
hdiutil convert "$RW_IMG" -format UDZO -o "$OUT" >/dev/null
rm -f "$RW_IMG"
rm -rf "$STAGE"
detach_volume "/Volumes/${VOL_NAME}"

echo "==> done: ${OUT}"
echo "    open \"${OUT}\""
