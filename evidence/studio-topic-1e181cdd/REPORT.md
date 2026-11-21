# Studio CSS topic receipt: logical scroll alignment

Source `1e181cdd7985b38e1a29988cf28d9b1c1521a1cf` (tree `952984637fe5b937fd16e61e1315f5a032073a26`), directly on `6e0a8f847fba9a4d55816f93ffbe5155f9f5ed17`. This is a topic measurement, not an integration result.

All 8,082 main + 41 supplement URLs occurred exactly once in the expected cohort; none of the 39 withheld URLs ran. The balanced 24-way partition replaces the previous partition. No baseline, control, crash or security replay. Full raw reports remain authoritative; status and observation coverage changes are not necessarily regressions or improvements.

| Cohort / reference | Strict pages +/− | Named passes +/− | Observed names +/− | Named statuses +/− | Page states +/− |
|---|---:|---:|---:|---:|---:|
| main / parent | 6/0 | 42/0 | 0/0 | 42/42 | 0/0 |
| main / frozen | 88/0 | 861/21 | 2/0 | 883/881 | 3/3 |
| main / chrome | 300/1328 | 5968/5593 | 260/498 | 11155/11393 | 150/150 |
| supplement / parent | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| supplement / frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| supplement / chrome | 0/8 | 0/42 | 0/0 | 42/42 | 0/0 |

Focus: 42 expected Fail→Pass; 18 expected residual sideways-lr failures; 6 expected strict-page gains. Full ordered mismatch URL lists vs focused receipts: {'parent': [], 'candidate': [], 'chrome': []}. Missing expected gains: []. Additional named gains: []; additional strict gained pages: [].

## Timing and comparability

Measured full primary wall 319.64 s versus 321 s recorded for the prior primary (-1.36 s). This is an observed difference, not a causal speedup. Main shard runner wall 41.794–53.788 s; supplement 0.459 s. Runner-reported CPU totals: browser 133.340 s, runner 68.200 s, server 0.000 s. Summed time(1) user/system 80.66/26.40 s (different accounting). Restarts 1; watchdog kills 0.

11 approximately 30-second host snapshots, average busy 13.57% (range 5.50–30.80%). CI listener/worker lines 11/0; sampling is not continuous. See host.log for memory, load and concurrent cargo/rustc.

Same patched runner files as the prior measurement, same WPT/JSC/Chrome pins (hashes in identities), 15/45 s defaults, full testharness, CSS, wptserve, testdriver, fresh contexts, two workers each, four runners in waves staggered by two seconds, worker globals only on separate supplement. Kernel ephemeral ports (range starts at 49152), outside reserved sibling ranges. Different grouping, launch times, host load, labels and binary; compare actual performance descriptively only. Reused topic binary byte-identical to the SHA-256 in the focused receipt at `192af50c` (3 Cargo jobs, isolated target); copied read-only, zero new builds. Prior primary used its own binary built with two Cargo jobs. Frozen/Chrome/parent maps reused, not regenerated.

## Evidence

The archive contains manifests, raw reports, command lines and timestamps, per-shard resource data, full directional sets, ordered parent differences, and the 87-name focus cross-check. `summary.json` includes candidate totals; `all-*.json.gz` keeps complete candidate-only and reference-only sets. Files in `refs` reproduce the pinned inputs. Run `shasum -a 256 -c SHA256SUMS` after extraction. To regenerate analysis in the extracted directory: `python3 -B analyze.py`.

Archive SHA-256: `c83c98e9ed054b751cb66dc744cb6055c5772f6c22fc98785375bc165e6d8a89`.
