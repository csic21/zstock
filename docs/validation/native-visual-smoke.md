# Native Linux visual smoke

The Linux Quality job builds the actual default-feature GPUI binary, reusing the
existing Cargo cache, then runs it on an isolated Xvfb display with Mesa lavapipe.
The existing offline Rust tests remain unchanged. This is a native rendering
smoke test, not a browser mock or a substitute for visual review.

## Evidence and scope

The `linux-native-visual-smoke` Actions artifact retains PNGs, the app/X11/Xvfb
logs, runner output, and anonymous navigation timings for 14 days, including on
failure. It covers:

- Today and Settings at 1320×860 and 800×860;
- the command palette over Settings at both sizes;
- Research, Opportunities, and Portfolio via their primary-toolbar shortcuts;
- Escape dismissal and repeated Settings open/close after dismissing the palette.

The script discovers only the window belonging to its app PID, bounds external
commands and window discovery, checks that the app stays alive, and terminates
its process group on exit. The workflow bounds the whole GUI run to three minutes.
The evidence checker decodes all ten PNGs, requires the expected dimensions and
nonblank content, detects logged asset/renderer failures, and verifies all four
navigation transitions in the app's own local task metrics.

PNG presence is **not** proof that every icon is painted or every control fits.
Missing assets can be silent in GPUI. Review the actual images for toolbar icons,
Chinese glyphs, selection/Back state, clipping, Settings rows, and palette placement
before claiming pixel correctness. Software rendering timings are not desktop
performance acceptance evidence; use the separate A6 protocol for that.

The app starts with built-in defaults in a new temporary `ZSTOCK_DATA_DIR` and
HOME. Its environment is cleared, and its D-Bus session address points to a
nonexistent private socket so native Secret Service cannot access real keys.
It uses no existing config, broker data, holdings, journals, AI keys, or CLI login.
The isolated profile is deleted, never uploaded. The app may fetch public market
quotes on startup; success does not depend on quote availability, so screenshots
are not deterministic market-data golden files. Credential-store unavailability
may appear in the status area and is expected for this run.

## Run on a normal Linux desktop/CI host

In addition to the build dependencies in the README, install the distro packages:

```sh
sudo apt-get install xvfb xauth xdotool imagemagick mesa-vulkan-drivers libvulkan1 fonts-noto-cjk
cargo build --locked --bin stock --target x86_64-unknown-linux-gnu
mkdir -p visual-smoke-artifacts
timeout --signal=TERM --kill-after=15s 180s \
  xvfb-run --auto-servernum --error-file=visual-smoke-artifacts/xvfb.log \
  --server-args="-screen 0 1600x1000x24 -nolisten tcp" \
  bash scripts/visual-smoke.sh \
  target/x86_64-unknown-linux-gnu/debug/stock visual-smoke-artifacts
```

Use a dedicated Xvfb display, not your active desktop. The script accepts either
the architecture-suffixed or unsuffixed distro lavapipe ICD filename. It uses
XTEST keys after explicitly focusing its own window, without requiring a window
manager. If the host forbids local X11 sockets, do not bypass that restriction;
run the native stage on the hosted Linux runner instead and report it as unrun
until its evidence is available.

The checker can be tested without a display or graphics stack:

```sh
bash -n scripts/visual-smoke.sh
python3 scripts/visual-smoke-check.test.py
```

These tests validate evidence handling only, not the native application.
