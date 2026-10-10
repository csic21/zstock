#!/usr/bin/env python3
"""Check native-smoke evidence; this is deliberately not a pixel-diff oracle."""

import json
from pathlib import Path
import re
import subprocess
import sys


SCREENS = {
    "today-1320": 1320,
    "settings-1320": 1320,
    "settings-palette-1320": 1320,
    "research-1320": 1320,
    "opportunities-1320": 1320,
    "portfolio-1320": 1320,
    "work-1320": 1320,
    "today-800": 800,
    "settings-800": 800,
    "settings-palette-800": 800,
    "returned-today-800": 800,
    "work-800": 800,
}


def check_image_info(info, expected_width):
    """ImageMagick decodes the PNG and reports size and distinct pixel colors."""
    image_format, width, height, colors = info.split()
    if image_format != "PNG" or (int(width), int(height)) != (expected_width, 860):
        raise ValueError(f"Unexpected screenshot format/size: {info}")
    if int(colors) < 16:
        raise ValueError(f"Screenshot appears blank ({colors} distinct colors)")


def check_log(log):
    failure = r"(?:failed|failure|error|missing|not found|unable|could not)"
    asset = r"\b(?:asset|svg|icon)s?\b"
    pattern = re.compile(
        rf"{failure}[^\n]{{0,160}}{asset}|{asset}[^\n]{{0,160}}{failure}"
        r"|panicked at|Failed to open window|NoSupportedDeviceFound",
        re.IGNORECASE,
    )
    match = pattern.search(log)
    if match:
        raise ValueError(f"App logged a renderer/asset failure: {match.group(0)}")


def check_navigation(metrics):
    tasks = [metric["task"] for metric in metrics]
    expected = ["today", "research", "opportunities", "portfolio"]
    if tasks != expected:
        raise ValueError(f"Native task shortcuts did not complete: {tasks}; expected {expected}")


def main():
    artifacts, metrics_path = map(Path, sys.argv[1:])
    check_log((artifacts / "app.log").read_text(errors="replace"))
    check_navigation(json.loads(metrics_path.read_text()))
    for name, width in SCREENS.items():
        path = artifacts / f"{name}.png"
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Missing or empty screenshot: {path.name}")
        info = subprocess.check_output(
            ["identify", "-format", "%m %w %h %k", str(path)],
            text=True,
            timeout=10,
        )
        check_image_info(info, width)
    print(f"Verified {len(SCREENS)} nonblank native captures and four task transitions.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        sys.exit(f"Visual smoke failed: {error}")
