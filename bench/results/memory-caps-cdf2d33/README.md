# Per-connection memory caps (measurement)

This directory holds the `memory_caps.txt` results. They come from 3 rounds of 5 scenarios, with one process per
scenario, run by `crates/lp-cdp/examples/memory_caps.rs` in a release build.

- **Source:** `main` cdf2d33 plus this branch's connection-wide `HandleBudget` (default 100,000).
- **Host:** Apple M1, 8 GB, macOS 26.7.1, rustc 1.99.1, `SDKROOT=MacOSX26.5.sdk`.
- **Changes after the measurement:** the example was edited only for lint/format (an unused helper was removed, a
  type alias added and a struct renamed). Its behavior is unchanged.
- **Metric:** the macOS `phys_footprint` delta of the process while the scenario's state is still retained. These
  are not browser or TOS memory figures.

An earlier version of the example ran all scenarios in one process. Its later scenarios reused memory that earlier
scenarios had freed (100,000 handles showed only +8.3 MiB). Those numbers were discarded and are not recorded here.

The interpretation is in `docs/automation/OPTIMIZATION.md`, under "Per-connection memory bounds".
