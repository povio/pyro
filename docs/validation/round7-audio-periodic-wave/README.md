# PeriodicWave: coefficient construction and oscillator rendering

Started 2026-11-22 08:35 CET on Studio at integrated `00e529058`. Source `0602e1268`. Census `01f49f43` identified `periodicWave.html` and `ctor-oscillator.html`. Their ordinary constructor, scheduling and small deterministic rendering tasks were inspected before execution. No withheld/crash fixtures.

| Scope | Baseline | Candidate | Native Chrome |
|---|---:|---:|---:|
| periodicWave.html | 15/29 | 31/31 | 31/31 |
| ctor-oscillator.html | 56/57 | 62/62 | 62/62 |
| Independent analytic PCM comparisons in the browser harness | 0/2 | 2/2 | 2/2 |
| Whole process wall time | 0.54 s | 0.54 s | 0.65 s |

WPT totals: 71/86 → 93/93, +22 passing assertions, zero previously passing names lost; both pages became strict passes. Denominators grow when previously aborted audit tasks finish. Some passing audit labels embed implementation-specific exception messages, so raw label strings are not identical to Chrome. PCM controls check non-silent samples, coefficient copying, DC exclusion and normalization on/off. The WPT output test alone is not sufficient evidence for PCM: its oscillator graph is not connected to the destination.

The pinned WPT checkout was `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Chrome `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` reported 158.0.8127.37; codesign verified, TeamIdentifier EQHXZ8M8AV. Each process ran sequentially over a pipe, loading the baseline or candidate's actual JS factory, or native constructors without injection. The harness clears native PeriodicWave in injected mode and verifies constructor identities on each page. No rebuilt Pyro binary or Rust runtime test. Reproduce from the source topic with `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$MODULE_PATH" periodic`; pass an empty module path for native. The baseline's nonzero exit is expected.

Independent reviewer: 15/15 Node VM controls passed; baseline passed two existing controls and lacked the interface for the rest. Directed controls included sine default, real/imaginary coefficient copying, DC, both normalization modes, oscillator option precedence and switching back to sine, Nyquist rejection, preserved lower harmonics, and exception handling. Reported maximum analytic PCM error ≤2.84e-8. No objection. This batch does not establish dense-wave performance, cross-realm compatibility or full oscillator conformance.

No Cargo slots, listening ports, new targets or deletions. Initial host load 3.20/2.27/1.83, final 1.31/1.60/1.66; memory free sampled at 84%, disk available 14 GiB. Raw named outcomes and process times adjacent.
