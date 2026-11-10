# 0013. 0013 Set CI benchmark thresholds for decrypt and decode

## Context
The targets on a normal laptop are: a jump shows the image in under 200 ms, and scrolling runs at 30 fps or more. GitHub runners are shared, 2-4 vCPU machines, roughly 1.5-3 times slower than a laptop and noisy.

## Options
- **Criterion with stored baselines:** too noisy on shared runners to compare between runs.
- **Fixed thresholds with generous margins, on medians and p95, in a release build.**

## Decision
The `timereel-bench` binary generates encrypted data at defaults. It takes 20 warm-up runs and 200 measured runs, prints a table (median, p95, max) and fails above these limits:

| Measurement | Median | p95 |
|---|---|---|
| Full image: decrypt + decode | 40 ms | 100 ms |
| Thumbnail: decrypt + decode | 5 ms | 10 ms |
| Cold jump through the loader (read + decrypt + decode) | - | 120 ms |

Argon2 unlock time is printed but not gated. Expected real values are about 10 ms and 1 ms.

## Consequences
The margins absorb runner noise but catch order-of-magnitude regressions. Environment variables can override the limits for local experiments; CI uses the defaults. The real feel (fps, under 200 ms) is checked by the owner at the merge gate.
