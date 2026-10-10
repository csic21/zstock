#!/usr/bin/env bash
# Run under a dedicated Xvfb display; see docs/validation/native-visual-smoke.md.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 /path/to/stock /path/to/artifacts" >&2
  exit 2
fi
for command in xdotool import identify python3 timeout setsid; do
  command -v "$command" >/dev/null || { echo "Missing command: $command" >&2; exit 2; }
done
[[ -n ${DISPLAY:-} ]] || { echo "Run this script through xvfb-run." >&2; exit 2; }
binary=$(realpath "$1")
[[ -x $binary ]] || { echo "Not an executable: $binary" >&2; exit 2; }
mkdir -p "$2"
artifacts=$(realpath "$2")
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
exec > >(tee "$artifacts/smoke.log") 2>&1

# Both names exist in supported distro versions. Require Mesa's software ICD;
# a hosted runner must not silently select an unavailable hardware GPU.
shopt -s nullglob
drivers=(/usr/share/vulkan/icd.d/lvp_icd*.json)
[[ ${#drivers[@]} -gt 0 ]] || { echo "Mesa lavapipe ICD not installed."; exit 2; }
driver=${drivers[0]}
isolated=$(mktemp -d -t zstock-visual-smoke.XXXXXXXX)
app_pid=
window=
cleanup() {
  result=$?
  trap - EXIT INT TERM
  if [[ -n $app_pid ]]; then
    # setsid gives this app its own process group, including any helper processes.
    kill -TERM -- "-$app_pid" 2>/dev/null || true
    for _ in {1..20}; do
      kill -0 "$app_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL -- "-$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  # Only anonymous task timings are retained. Never archive HOME or config.
  if [[ -f $isolated/data/task-metrics.json ]]; then
    cp "$isolated/data/task-metrics.json" "$artifacts/task-metrics.json"
  fi
  rm -rf -- "$isolated"
  echo "Visual smoke exit status: $result"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$isolated"/{home,config,cache,runtime,data}
chmod 700 "$isolated/runtime"

# Recovery fixture is synthetic and disposable. Never point this script at an
# existing profile. Exercise automatic startup/quote activity without allowing
# unreadable financial documents to turn into empty writable defaults.
recovery_mode=${ZSTOCK_SMOKE_RECOVERY:-0}
case "$recovery_mode" in
  0|portfolio|journal|both|recovered) ;;
  1) recovery_mode=both ;;
  *) echo "Unknown synthetic recovery case: $recovery_mode"; exit 2 ;;
esac
for document in portfolio journal; do
  if [[ $recovery_mode == "$document" || $recovery_mode == both || $recovery_mode == recovered ]]; then
    printf 'synthetic broken %s fixture\n' "$document" > "$isolated/data/$document.json"
    cp "$isolated/data/$document.json" "$isolated/expected-$document"
  fi
done


# A brand-new data directory selects the app's built-in defaults. env -i prevents
# inherited AI credentials/CLI configuration from entering the app. HOME/data
# isolation alone does NOT isolate native Secret Service: use a nonexistent bus.
setsid env -i \
  PATH=/usr/bin:/bin LANG=C.UTF-8 TZ=UTC \
  HOME="$isolated/home" XDG_CONFIG_HOME="$isolated/config" \
  XDG_CACHE_HOME="$isolated/cache" XDG_DATA_HOME="$isolated/data" \
  XDG_RUNTIME_DIR="$isolated/runtime" ZSTOCK_DATA_DIR="$isolated/data" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=$isolated/no-session-bus" \
  DISPLAY="$DISPLAY" XAUTHORITY="${XAUTHORITY:-}" \
  VK_DRIVER_FILES="$driver" VK_ICD_FILENAMES="$driver" \
  LIBGL_ALWAYS_SOFTWARE=1 RUST_BACKTRACE=1 \
  "$binary" >"$artifacts/app.log" 2>&1 &
app_pid=$!
echo "Started native ZStock (PID $app_pid), Vulkan ICD: $driver"

alive() {
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "ZStock exited unexpectedly; see app.log."
    tail -n 80 "$artifacts/app.log"
    exit 1
  fi
}
xdo() { timeout --kill-after=2s 5s xdotool "$@"; }

# Restrict discovery to this exact app process, not another ZStock on the host.
deadline=$((SECONDS + 45))
while (( SECONDS < deadline )); do
  alive
  windows=$(xdo search --onlyvisible --all --pid "$app_pid" --name '^ZStock' \
    2>>"$artifacts/x11.log" || true)
  if [[ -n $windows ]]; then
    read -r window <<< "$windows"
    [[ $windows == "$window" ]] || { echo "Expected one ZStock window, found: $windows"; exit 1; }
    break
  fi
  sleep 0.25
done
[[ -n $window ]] || { echo "No visible ZStock window within 45 seconds."; exit 1; }
[[ $(xdo getwindowpid "$window") == "$app_pid" ]] || { echo "Window PID mismatch."; exit 1; }
xdo windowmove "$window" 0 0
xdo windowsize --sync "$window" 1320 860
xdo windowfocus --sync "$window"
# Give cold software-renderer/font initialization a bounded settling period.
sleep 3

capture() {
  alive
  sleep 1
  timeout --kill-after=2s 10s import -window "$window" "$artifacts/$1.png"
  [[ -s $artifacts/$1.png ]] || { echo "Empty screenshot: $1"; exit 1; }
  alive
  echo "Captured $1.png"
}
key() {
  alive
  xdo windowfocus --sync "$window"
  # Send real XTEST events to the focused window, rather than synthetic
  # --window SendEvent events that toolkits may deliberately ignore.
  xdo key --clearmodifiers "$@"
  sleep 0.5
  alive
}

if [[ $recovery_mode == recovered ]]; then
  capture before-recovery-today-1320
  key ctrl+comma
  capture before-recovery-settings-1320
  # Only this fresh synthetic profile is reset. Both buttons use the first
  # recovery row: the remaining journal row moves there after portfolio reset.
  # Each operation requires two real clicks, just like explicit user recovery.
  for document in portfolio journal; do
    xdo mousemove --window "$window" 600 350 click 1
    sleep 0.3
    xdo mousemove --window "$window" 600 350 click 1
    deadline=$((SECONDS + 10))
    until python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$isolated/data/$document.json" 2>/dev/null; do
      alive
      (( SECONDS < deadline )) || { echo "Synthetic $document recovery did not complete"; exit 1; }
      sleep 0.2
    done
    sleep 1
  done
  capture after-recovery-settings-1320
  key Escape
fi

capture today-1320
key ctrl+comma
capture settings-1320
key ctrl+k
capture settings-palette-1320
key Escape
key Escape

# Exercise the toolbar's four primary-task shortcuts and their native dispatch.
# The checker also requires the resulting anonymous task-transition records.
key ctrl+2
capture research-1320
key ctrl+3
capture opportunities-1320
key ctrl+4
capture portfolio-1320
key ctrl+shift+w
xdo windowsize --sync "$window" 1320 860
capture work-1320
key ctrl+shift+w
key ctrl+1
xdo windowsize --sync "$window" 800 860
capture today-800
key ctrl+comma
capture settings-800
key ctrl+k
capture settings-palette-800
key Escape
key Escape
# Repeated open/close must remain responsive after the palette was dismissed.
key ctrl+comma
key ctrl+comma
capture returned-today-800
key ctrl+shift+w
xdo windowsize --sync "$window" 800 860
capture work-800
key ctrl+shift+w

alive
for document in portfolio journal; do
  if [[ -f $isolated/expected-$document ]]; then
    if [[ $recovery_mode == recovered ]]; then
      python3 - "$isolated" "$document" <<'PY'
import json, sys
from pathlib import Path
root, document = Path(sys.argv[1]), sys.argv[2]
json.loads((root / 'data' / f'{document}.json').read_text())
original = (root / f'expected-{document}').read_bytes()
assert any(path.read_bytes() == original for path in (root / 'data').glob(f'{document}.json.recovery.*')), 'missing preserved original bytes'
PY
      echo "PASS: explicit synthetic $document recovery preserved the original bytes"
    else
      cmp "$isolated/data/$document.json" "$isolated/expected-$document"
      echo "PASS: unreadable $document fixture bytes remain unchanged"
    fi
  fi
done
python3 "$script_dir/visual-smoke-check.py" "$artifacts" "$isolated/data/task-metrics.json"
echo "Native smoke checks passed. Review PNGs for icon visibility and layout correctness."
