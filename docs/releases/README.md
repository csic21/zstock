# Release publication

The release workflow supports three entry points:

- Push a stable `vX.Y.Z` tag at the current `main` commit. Its version must match `Cargo.toml`.
- Push a request-only commit to `main`, after quality checks pass for its parent. Add or modify only `.github/release-request.json` with this schema:

  ```json
  { "schema_version": 1, "tag": "v0.0.58", "source_sha": "<40-character parent commit SHA>" }
  ```

  The request must have exactly one parent, equal to `source_sha`. The workflow verifies the source version, creates the tag only if absent, and refuses an existing tag pointing elsewhere. The same run tests and builds that source; a tag created with `GITHUB_TOKEN` does not need to trigger a second run.
- Run `workflow_dispatch` to build artifacts only, without creating a tag or publishing a Release.

All four quality targets and all platform packaging jobs must finish before publication. The final job collects nine packages, uploads a draft Release, and validates every asset's state, size, and server-reported SHA-256. Only then is the Release made public/latest. Its four ZIP hashes feed `updates/stable.json`.

The stable manifest is committed on top of the exact publication head (the tag source or request-only commit). A non-force ref update prevents a concurrent `main` change from being overwritten. A concurrent change after publication can leave the complete Release public while the manifest remains unchanged; inspect the failed job rather than force-pushing. Retry a failed final job against the same build artifacts when appropriate. Existing published assets are never replaced by the helper, and mismatching assets fail closed.

Release notes are read from `docs/releases/<tag>.md` when present, otherwise generated from GitHub. The committed stable manifest stays on the previous version until the real release assets have been verified.

## One-release Git timestamp override

`.github/release-commit-dates.json` contains an explicit override for `v0.0.58` only: the manifest commit's author and committer date is `2026-10-09T08:59:00+08:00`, within that day's pre-09:00 window. It must be strictly after both source and manifest-parent author/committer dates. The helper refuses publication if this cannot be satisfied. Other tags omit explicit Git dates and retain GitHub's normal timestamp behavior.

This changes Git commit metadata only. It does not change or represent the actual Actions execution, upload, or Release publication times.

Run the lightweight release regressions with:

```sh
node --test scripts/*release.test.cjs
```
