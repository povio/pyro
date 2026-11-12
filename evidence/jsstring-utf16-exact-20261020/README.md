# `JsString::new` UTF-16 sizing: synthetic paired benchmark

This is a synthetic `JsString::new` workload in `crates/pyro-js/examples/large_string_cycles.rs`. It has no DOM, network, CDP, browser or TOS component. ES modules are compiled from UTF-8 by `pyro_jsc_module_script_create` and never pass through `JsString::new`. Nothing here establishes how much TOS or any real page benefits.

## Variants

All three variants are built from the same tree and example. Only `crates/pyro-js/src/string.rs` differs. `PROVENANCE.json` lists file and binary SHA-256s, host, toolchain and SDK.

- **`base`** (`2b4d66b`): `s.encode_utf16().collect()`.
- **`len`** (`a32656e`): `Vec::with_capacity(s.len())`. This over-allocates up to 3× for 3-byte UTF-8.
- **`exact`** (this branch): `Vec::with_capacity(utf16_len(s))`.
  - `utf16_len` has an ASCII fast path.
  - Otherwise it counts one unit per non-continuation byte, plus one for each 4-byte lead byte.
  - The count is accumulated in 127-byte `u8` chunks, so it vectorizes.

## Method

- **Order:** `run.sh` runs 9 configurations. Each configuration has 6 rounds, and each round uses a different one of the 6 permutations of (base, len, exact). Every variant therefore runs first, second and third equally often. `ORDER.tsv` records the actual order and each run's completion time (UTC, 1-second resolution, taken after the run exits).
- **Raw output:** every per-cycle line is in `raw/`. `summarize.py` produces `SUMMARY.json`.
- **Cycles:** each run does 30 cycles.
  - `convert` converts the same source and drops the result.
  - `realm` creates a realm, evaluates the source, disposes the realm, then runs a synchronous full GC and drains finalized payloads. The run aborts if the GC SPI is unavailable.
- **Text:**
  - `ascii` is ASCII only.
  - `cjk` has 3-byte scalars in long literals: about 4.19 MB UTF-8 becomes 1.86 M UTF-16 units.
- **Metrics:**
  - **Footprint and RSS:** the increase of `ri_phys_footprint` and `ri_resident_size` over the loop, in MiB.
  - **CPU:** `getrusage(RUSAGE_SELF)` user+system seconds. The `timeval` is the Darwin layout: `i64` seconds and `i32` microseconds, size-checked at compile time.
  - **Wall:** monotonic seconds.
- **Not sampled:** host CPU utilization.

## Results (median of 6, min–max in parentheses)

| Configuration | base footprint MiB | len | exact | base CPU s | len | exact |
|---|---:|---:|---:|---:|---:|---:|
| realm 4 MiB ascii | 416.9 (414.5–420.3) | 183.5 (180.5–188.3) | 180.6 (179.8–183.4) | 0.686 | 0.672 | 0.675 |
| realm 4 MiB cjk | 159.8 (157.4–160.9) | 54.5 (53.2–56.9) | 56.0 (53.7–56.9) | 0.227 | 0.220 | 0.230 |
| convert 4 MiB ascii | 249.1 | 16.1 | 16.1 | 0.106 | 0.091 | 0.093 |
| convert 4 MiB cjk | 111.0 | 7.2 | 7.2 | 0.085 | 0.078 | 0.088 |
| convert 12 MiB cjk | 21.5 | 21.5 | 21.5 | 0.235 | 0.237 | 0.267 |
| realm 1 MiB ascii (control) | 52.9 | 52.0 | 52.4 | 0.179 | 0.178 | 0.179 |
| realm 1 MiB cjk (control) | 15.8 | 15.8 | 15.8 | 0.066 | 0.066 | 0.069 |
| convert 1 MiB ascii (control) | 5.4 | 4.0 | 4.0 | 0.024 | 0.022 | 0.023 |
| convert 1 MiB cjk (control) | 1.8 | 1.9 | 1.8 | 0.019 | 0.020 | 0.022 |

RSS tracks footprint to within about 1 MiB; see `SUMMARY.json`. Wall time tracks CPU.

## Reading

- **Memory.** Exact sizing keeps the full footprint reduction wherever `base` retained memory, for both ASCII and 3-byte-heavy text. `base` retention depends on size and is not monotonic: 12 MiB CJK and the 1 MiB controls show none.
- **Against the replaced `a32656e` head (`len`):**
  - **CPU.** The counting pass makes CJK conversion about 13% slower:
    - 4 MiB CJK: 0.078 → 0.088 s over 30 conversions, about +0.33 ms per conversion.
    - 12 MiB CJK: 0.237 → 0.267 s, about +1 ms per conversion.
    - Realm 4 MiB CJK: 0.220 → 0.230 s.
    - ASCII is unchanged within noise (fast path).
  - **Footprint.** No measured improvement over `len` in these results: 7.2 vs 7.2 MiB (convert 4 MiB CJK) and 56.0 vs 54.5 MiB medians with overlapping ranges (realm 4 MiB CJK).
  - **Benefit.** Exact sizing avoids `len`'s over-allocated transient capacity: up to 3× for 3-byte UTF-8 and 2.25× for this CJK source. It does not reduce measured retention further.
- **Mechanism.** The macOS allocator mechanism behind `base`'s retention is not established. In an earlier probe, `malloc_zone_pressure_relief` released 0 MiB of it.

Reproduce:
1. Build the three binaries as described in `PROVENANCE.json`.
2. `./run.sh BIN_DIR OUT_DIR`
3. `python3 -I summarize.py OUT_DIR`
