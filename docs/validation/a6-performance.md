# A6 performance validation

The application records anonymous local performance evidence in
`performance-report.json` under the ZStock application-data directory. The report contains only
timings, RSS bytes, timestamps and sample counts. It does not contain security codes, positions,
amounts, notes or journal text.

## Instrumented metrics

- process start to first interactive render;
- cached primary-task interaction to the GPUI post-render callback, with p95;
- UI element-tree build duration, with p95 and p99;
- a deterministic 120-interaction chart zoom sequence bracketed by machine-readable log markers;
- resident set size sampled once per minute;
- RSS growth from the first settled sample at or after 60 seconds, only after the comparison sample
  is at least 3,600 seconds later; the report includes both baseline time and actual window length.

For the chart budget, launch with `ZED_MEASUREMENTS=1` and calculate p95/p99 from GPUI's own
`frame duration` lines between `A6_CHART_FRAMES_BEGIN` and `A6_CHART_FRAMES_END`. GPUI measures
these values around `window.draw(cx); window.present();`, so they cover CPU draw and present
submission. They do not claim to measure when the GPU has physically completed execution. The
UI-build metric remains separate and is not used as a substitute for chart-frame duration.

## Preliminary release run

One macOS arm64 release launch on 2026-08-09 produced:

| Metric | Result | Budget | Evidence status |
|---|---:|---:|---|
| First interactive render | 327.76 ms | ≤ 1,500 ms | preliminary pass; one sample |
| UI build | 0.74 ms | p95 ≤ 16.7 ms, p99 ≤ 33 ms | preliminary pass; two samples |
| RSS | 101,548,032 bytes | ≤ 250 MiB | preliminary pass; one sample |
| Cached navigation | — | p95 ≤ 100 ms | awaiting user navigation samples |
| Chart interaction frame | — | p95 ≤ 16.7 ms, p99 ≤ 33 ms | awaiting rendered-frame samples |
| One-hour RSS growth | — | ≤ 10% | awaiting a continuous one-hour run |

## Beta acceptance procedure

1. Build the release binary.
2. Create a new empty validation directory outside the normal application-data directory.
3. Launch with `ZSTOCK_DATA_DIR=/absolute/validation/path`, `ZSTOCK_A6_VALIDATE=1`, and
   `ZED_MEASUREMENTS=1`. Validation mode refuses to start without the isolated data directory.
4. The opt-in exercise rotates Today, Research, Opportunities and Portfolio at least 20 times,
   waits for real chart data, then performs 120 zoom interactions. If chart data is not ready after
   30 seconds, it records an error and does not manufacture empty-chart samples.
5. Leave the RSS validation process running until at least 3,600 seconds after its first settled
   sample at or after 60 seconds (normally at least 3,660 seconds total process time).
6. Inspect `performance-report.json`. Require `validation_mode: true`, at least 20 navigation
   samples, `validation_chart_interaction_count: 120`, at least 62 RSS samples, non-null one-hour
   growth, `rss_growth_baseline_elapsed_secs >= 60`, `rss_growth_window_secs >= 3600`, and all four
   in-app budget helpers to pass. Insufficient navigation or RSS evidence produces no budget result
   rather than a false pass.
7. From the process log, select GPUI `frame duration` values between the two A6 markers and require
   at least 100 samples, p95 ≤ 16.7 ms and p99 ≤ 33 ms.
8. Platform GPU tooling may be captured as supplementary evidence, but must be labelled separately
   from GPUI's draw/present-submission duration and the UI-build metric.
