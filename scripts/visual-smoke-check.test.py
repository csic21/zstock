#!/usr/bin/env python3
"""Offline tests of the evidence checker; these do not launch or simulate GPUI."""

import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location(
    "visual_smoke_check", Path(__file__).with_name("visual-smoke-check.py")
)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class EvidenceChecks(unittest.TestCase):
    def test_expected_native_image_sizes(self):
        check.check_image_info("PNG 1320 860 2500", 1320)
        check.check_image_info("PNG 800 860 2000", 800)

    def test_blank_wrong_size_or_wrong_format_fails(self):
        for info in ["PNG 800 860 1", "PNG 1320 860 2000", "JPEG 800 860 2000"]:
            with self.subTest(info=info), self.assertRaises(ValueError):
                check.check_image_info(info, 800)

    def test_logged_asset_and_renderer_errors_fail(self):
        for log in [
            "failed to load SVG: icons/settings.svg",
            "asset not found: icons/search.svg",
            "missing icon: eye",
            "thread 'main' panicked at renderer.rs:42",
            "NoSupportedDeviceFound",
        ]:
            with self.subTest(log=log), self.assertRaises(ValueError):
                check.check_log(log)

    def test_quote_failures_are_not_icon_failures(self):
        check.check_log("quote network error: timeout\nloading SVG assets\n")

    def test_navigation_requires_all_four_native_transitions_in_order(self):
        tasks = ["today", "research", "opportunities", "portfolio"]
        check.check_navigation([{"task": task} for task in tasks])
        for incomplete in [[], tasks[:-1], tasks[::-1], tasks + ["today"]]:
            with self.subTest(tasks=incomplete), self.assertRaises(ValueError):
                check.check_navigation([{"task": task} for task in incomplete])


if __name__ == "__main__":
    unittest.main()
