# Native credential-store validation

The ignored `native_secret_round_trip_smoke` test creates a randomized account, writes a fixed
non-production value, reads it back, deletes it and confirms it is absent. No user API key is read
or modified.

| Platform | Backend | Status | Evidence |
|---|---|---|---|
| macOS arm64 | Keychain via `security` | Passed 2026-08-09 | Native ignored test: 1 passed |
| Linux | Secret Service via `secret-tool` | Pending | Must run in a desktop session with an unlocked collection |
| Windows | DPAPI via PowerShell secure strings | Pending | Must run under the same Windows user for write/read/delete |

The `credential-smoke.yml` workflow runs the same test on macOS, Windows and a temporary Linux
D-Bus/GNOME Keyring session. It is scheduled separately from PR quality gates and can also be
started manually.

Platform command:

```text
cargo test native_secret_round_trip_smoke -- --ignored --nocapture
```

The macOS nightly workflow also executes ignored tests. Linux and Windows are intentionally not
claimed as verified until the native round trip runs on those operating systems.
