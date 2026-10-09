# Vendored GPUI Component hover fix

This directory contains the exact published `gpui-component` 0.5.1 crate with one runtime source change. It stays on the app's existing component version.

- Registry: https://crates.io/crates/gpui-component/0.5.1
- Upstream: https://github.com/longbridge/gpui-component
- Published source commit: `0f0ab35233212f8f3277028995caf0c41e13ee6c`, path `crates/ui` (see `.cargo_vcs_info.json`).
- Original `.crate` SHA-256: `d021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a` (verified against the downloaded registry archive).
- License: Apache-2.0; original `LICENSE-APACHE`, README, normalized and original manifests retained.

## Patch

In `src/button/button.rs`, the hover builder now uses `hover_style.fg` instead of hardcoded `crate::red_400()`. This restores the foreground calculated by each button variant, including primary, secondary, ghost and custom styles. No other upstream runtime source is changed.

`Cargo.toml` uses a `[patch.crates-io]` entry scoped only to `gpui-component`. The application remains responsible for registering `gpui-component-assets`.

Cargo's extraction marker `.cargo-ok`, any generated `.cargo-checksum.json`, and the library crate's standalone `Cargo.lock` are omitted. Source files, locales, upstream test fixtures, manifests and license remain unchanged apart from the single-line patch. The application-level lockfile controls dependency resolution.

## Verification

`app::ui::chrome::interaction_tests::hovered_buttons_keep_the_variant_foreground` paints real Button children and inspects their inherited foreground before and after pointer hover. It is intended to fail with the original hardcoded red line, rather than only testing a color helper.

Remove this override once an explicitly tested upstream release includes the fix; do not edit Cargo's registry cache as a substitute.
