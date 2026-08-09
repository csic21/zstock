# A6 performance validation

The application records anonymous local performance evidence in
`performance-report.json` under the ZStock application-data directory. The report contains only
timings, RSS bytes, timestamps and sample counts. It does not contain security codes, positions,
amounts, notes or journal text.

## Instrumented metrics

- process start to first interactive render;
- cached primary-task state transition duration;
- UI element-tree build duration, with p95 and p99;
- resident set size sampled once per minute;
- RSS growth only after two samples are at least 3,600 seconds apart.

The UI-build metric is not a substitute for GPU presentation time. The A6 chart-frame budget must
still be confirmed with platform frame tooling during Beta.

## Preliminary release run

One macOS arm64 release launch on 2026-08-09 produced:

| Metric | Result | Budget | Evidence status |
|---|---:|---:|---|
| First interactive render | 327.76 ms | ≤ 1,500 ms | preliminary pass; one sample |
| UI build | 0.74 ms | p95 ≤ 16.7 ms, p99 ≤ 33 ms | preliminary pass; two samples |
| RSS | 101,548,032 bytes | ≤ 250 MiB | preliminary pass; one sample |
| Cached navigation | — | p95 ≤ 100 ms | awaiting user navigation samples |
| One-hour RSS growth | — | ≤ 10% | awaiting a continuous one-hour run |

## Beta acceptance procedure

1. Build and launch the release binary normally.
2. Exercise Today, Research, Opportunities and Portfolio from cached state at least 20 times.
3. Use charts continuously across A-share and Hong Kong symbols.
4. Leave the app running for at least 3,600 seconds.
5. Inspect `performance-report.json`; all four budget helpers require real samples and return no
   result when evidence is insufficient.
6. Capture GPU frame p95/p99 with the platform profiler; do not label UI-build time as frame time.
