# Performance

This page collects the measurements the project has published, says where each comes from, and explains how to measure your own suite and which levers move it. All numbers were taken on Apple silicon Macs. No Linux numbers have been published yet.

## Published measurements

### Per test

The reference flow is the "closes an open position" test of TOS, the application Lightningpanda was first built to test, run against a TOS-shaped app in the repository's benchmark site. Median wall time per test:

| How the test is driven | Time per test | Source |
| --- | ---: | --- |
| Native `LP.run`, booted page, 4 connections, back to back | 2.75 ms | `bench/results/lp-tos-e501cef` |
| Native `LP.run`, fresh page | 8.6 ms | `bench/results/lp-tos-e501cef` |
| Stock Playwright over CDP, same browser | 34.2 ms | `bench/results/lp-tos-e501cef` |
| Playwright module (`@povio/lightning-panda/playwright`), Playwright code as TOS writes it, fresh page | 14.7 ms | package README |
| Stock Playwright over CDP, same flow | 41-44 ms | package README |

Conditions for `lp-tos-e501cef`: Apple M1 Max, 10 cores, release build, 5 rounds of 15 samples, on a busy machine (load average about 13), so differences under 10-15% are noise.

What the table shows: the same browser is about 12 times faster per test when the whole scenario is one native command on a booted page, and about 3 times faster when Playwright code runs through the Playwright module.

### Fork mode

From `docs/ARCHITECTURE.md`, M1 Max, release build:

| Measurement | Result |
| --- | ---: |
| An app whose startup runs 0.8 s of JavaScript, handed to a new connection already booted | about 20 ms |
| Connect to an evaluated expression in a booted page, spare fork waiting (8 MB test app) | 3.6 ms |
| Module precompilation on the full TOS suite (2,160 tests): instructions per page | 3,079 M without, 2,816 M with (-8.5%) |

### Memory

| Measurement | Result | Source |
| --- | --- | --- |
| 20-test loop on a 2 MB app, memory released on page close | plateaus at 275 MB instead of 323 MB | repository README |

### Against upstream Lightpanda

A matched HTTP workload (50 navigations and 20 context cycles, three fresh processes per binary, 2026-10-17) in `bench/results/http-cpu-d673567`:

| Median of 3 | Lightpanda (upstream) | Lightningpanda |
| --- | ---: | ---: |
| 50 navigations, browser CPU | 283.5 ms | 77.9 ms |
| 20 context cycles, browser CPU | 73.2 ms | 49.5 ms |
| Peak resident memory | 32.0 MB | 15.9 MB |

This workload has no DOM interaction, subresources or TLS. It measures the navigation path, not whole-browser parity.

## Measure your own suite

Wall time on a shared laptop or CI runner is noisy. Use these, from cheapest to most precise:

1. **Run the suite twice with each setup and compare totals.** Alternate the setups (A, B, A, B) so machine load hits both equally.
2. **Read `RunResult.ms`.** Every `page.run` reply carries the time spent in the browser. Compare it with the wall time of the test to see how much goes to your test code, seeding and the network.
3. **Read `lp.stats`.** It counts pages taken booted (`prepared`), taken while still booting (`preparedBooting`) and booted for the test (`fresh`). Many `fresh` pages mean the pool is too small for the pace of your tests.
4. **Measure CPU, not wall time, under load.** On macOS, `/usr/bin/time -l` prints the CPU time and peak memory of a command and the child processes it waited for. In fork mode, `LIGHTNINGPANDA_FORK_STATS=<file>` logs each forked page's CPU time, page faults and instructions.

```sh
/usr/bin/time -l bunx lightningpanda e2e e2e --concurrency 8
```

When comparing against Chrome, compare the same tests on the same machine, with the same concurrency, in the same session.

## Levers

In rough order of effect:

| Lever | What it removes | How |
| --- | --- | --- |
| Native runs | Round trips and client-side polling: one command per scenario instead of one per call | Rewrite slow tests as `page.run([...])` ([Porting from Playwright](/e2e/porting)) |
| Profiles | The app's boot from each test | `lp.profile()` and `lp.page({ as, route })`, or `use.lightningpanda.boot` in the Playwright module ([Profiles](/guide/profiles)) |
| Fork mode | Boot cost entirely, and per-test cleanup | `serve --fork`, or `lightningpanda e2e`, which uses it automatically ([Fork mode](/guide/fork-mode)) |
| Several connections | Waiting behind another page's boot on the same thread | `connect(ws, { connections: 4 })` in thread mode |
| Static serving | Your dev server's response time, and its CPU | `--static http://localhost:3000=dist` serves a production build from disk ([Serving a build](/e2e/static)) |
| `setup` snapshots | Repeated sign-in and navigation steps | `setup` and `base` profiles in fork mode |
| Seeding through the API | Clicking through forms to create test data | create data with your API, then assert in the UI |

What does not help: raising timeouts, adding waits, and splitting a scenario into one `run` per step. Each extra `run` is a round trip, and request checks only see their own run.

### Profile-guided optimization

The repository has a profile-guided optimization (PGO) build script for **source builds**, trained on a Playwright suite, a CSS benchmark and, optionally, your own workload. On an M1 Max it measured 5-7% less CPU on the Playwright suite and 5-16% on DOM edit and layout loops, with PDF output byte-identical. The npm package's binaries are not built with PGO. See the repository README if you build from source.
