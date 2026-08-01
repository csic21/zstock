#!/usr/bin/env bash
# Install ZStock into ~/.local with desktop integration (Zed-style).
#
#   ./install.sh            # install
#   ./install.sh --uninstall
#
# Override the prefix for testing/system-wide installs:
#   ZSTOCK_PREFIX=/opt/zstock ./install.sh
set -euo pipefail

PREFIX="${ZSTOCK_PREFIX:-$HOME/.local}"
APP_NAME="ZStock"
BIN_NAME="stock"
DESKTOP="zstock.desktop"
ICON_NAME="zstock"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN_SRC="${SCRIPT_DIR}/${BIN_NAME}"
DESKTOP_SRC="${SCRIPT_DIR}/${DESKTOP}"
ICON_DIR_SRC="${SCRIPT_DIR}/icons/hicolor"

fail() {
  echo "错误：$*" >&2
  exit 1
}

if [ ! -x "${BIN_SRC}" ]; then
  fail "未找到可执行文件 ${BIN_SRC}（请确保在解压后的目录里运行）"
fi
[ -f "${DESKTOP_SRC}" ] || fail "缺少 ${DESKTOP_SRC}"
[ -d "${ICON_DIR_SRC}" ] || fail "缺少图标目录 ${ICON_DIR_SRC}"

uninstall() {
  echo "==> 卸载 ZStock（保留配置）"
  rm -f "${PREFIX}/bin/${BIN_NAME}"
  rm -f "${PREFIX}/share/applications/${DESKTOP}"
  find "${PREFIX}/share/icons/hicolor" -path "*/apps/${ICON_NAME}.png" -delete 2>/dev/null || true
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${PREFIX}/share/applications" 2>/dev/null || true
  fi
  echo "==> 完成。应用配置保留在 ~/.local/share/stock-analysis/。"
}

if [ "${1:-}" = "--uninstall" ]; then
  uninstall
  exit 0
fi

echo "==> 安装 ZStock 到 ${PREFIX}"
mkdir -p "${PREFIX}/bin" "${PREFIX}/share/applications" "${PREFIX}/share/icons/hicolor"

cp "${BIN_SRC}" "${PREFIX}/bin/${BIN_NAME}"
chmod +x "${PREFIX}/bin/${BIN_NAME}"

# 把 Exec 占位符替换成绝对路径，避免依赖 PATH
sed "s|@EXEC@|${PREFIX}/bin/${BIN_NAME}|" "${DESKTOP_SRC}" > "${PREFIX}/share/applications/${DESKTOP}"

# 图标按 hicolor 规范安装（可被主题系统识别）
while IFS= read -r size_png; do
  [ -n "${size_png}" ] || continue
  size_dir="${size_png%/apps/*}"
  size_name="${size_dir##*/}"
  mkdir -p "${PREFIX}/share/icons/hicolor/${size_name}/apps"
  cp "${size_png}" "${PREFIX}/share/icons/hicolor/${size_name}/apps/${ICON_NAME}.png"
done < <(find "${ICON_DIR_SRC}" -name "${ICON_NAME}.png")

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "${PREFIX}/share/applications" 2>/dev/null || true
fi

echo "==> 完成"
echo "    启动：${PREFIX}/bin/${BIN_NAME}（或在应用菜单里找 ${APP_NAME}）"
echo "    卸载：${SCRIPT_DIR}/install.sh --uninstall"
case ":${PATH}:" in
  *":${PREFIX}/bin:"*) ;;
  *) echo "    提示：${PREFIX}/bin 不在 PATH 中，可执行：export PATH=\"${PREFIX}/bin:\$PATH\"" ;;
esac
