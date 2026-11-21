# Cancel a stale transition on endpoint change

The archived batch14/batch15 A/B/B/A repeats showed four repeatable width losses at progress 0.75 on `calc-size-interpolation-expansion.html`. The page reuses the element: it removes the transition shorthand, flushes a new starting value, then enables a new transition. Batch15 sampled the previous run on retarget even though that run should already have been canceled. The defect is in the [CSS Transitions starting/canceling algorithm](https://drafts.csswg.org/css-transitions/#starting), not calc-size or the reversal shortening calculation. The correction checks running endpoints against the after-change style even when no replacement starts. Removing the shorthand without changing the endpoint still retains the object under the default `transition-property: all`.

Source: `8202ed42c23cadad1aaec76448e29271fb468e36` on topic parent `3961664ab48bd449c696ab56ac8fd0c2aa6b8416`. The tested candidate is the immutable integrated batch15 source `ce79ed2cf2f71d8c9b8b38846470a0515a7c0df3` plus exactly this patch (git diff SHA-256 `162134d08ceb867428d2199fb38374bc1c2f5bf86070f5a111250417e759900a`; resulting animation source byte-identical). Batch14 source `8627d988ed60b15992282350b5c17d97cb39d9e7`.

| Candidate vs reference | Strict pages gained/lost | Passing names gained/lost |
|---|---:|---:|
| Fresh batch15 parent | +1 / −0 | +6 / −0 |
| Archived batch15 | +1 / −0 | +6 / −0 |
| Archived batch14 | +4 / −0 | +5 / −0 |
| Pinned Chrome 158.0.8127.37 | +0 / −2 | +0 / −281 |

All nine allowlisted pages completed with the same 1,929 assertion occurrences. The candidate recovers the four expansion cases and also passes both previously failing cancellation assertions (zero combined duration and non-interpolable endpoint). All three batch15 transition gains over batch14 remain: currentTime reversal, replaced effect reversal, and retargeting with box sizing. Width and height interpolation controls have no directional losses against either Pyro reference; these are the two pages that still fall short of Chrome. These comparisons cover only `urls.json`, not the full cohort.

The independent opacity control (`control.js`) distinguishes cancellation from coincidentally correct output: all retain the run while the endpoint stays unchanged. On endpoint change with transitions disabled, batch14 and batch15 still list one run; Chrome and candidate list zero. The next sample is 0.85 on Chrome/candidate and 0.95 on batch15. Candidate and Chrome control JSON are byte-identical, including repeated reversal and unrelated retarget durations (100000, 25000, 87500, 100000 ms) and positive/negative delay cases. `probe.cjs` drives the control on Pyro over CDP.

One warm patched-JSC build (`0e0dd7038a7a-6f4649e395e0`, wptiter, render, 3 Cargo workers): 183 s. Binary SHA-256: batch14 `6cd1051702a04a283ccf8cfd80ca305ae9b4f7bebd738f90cd9d5a64858edd87`, batch15 `d920e1e9f0f17fae9afeadde712d9af666134f0b2be13a05aefcadac50a38eda`, candidate `db2a1d6d4822b78a86355bca8e402157eac4b54fc3cc97304de82523e0fd3991`. Focused WPT (`a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, wptserve, testdriver, two workers): fresh read-only parent 947 ms (port 49587), read-only candidate 885 ms (65163); zero restarts or watchdog kills. Chrome WPT comparison uses the archived pinned rows, not a new full run.

Run `python3 compare.py` from this directory to regenerate `directions.json.gz`'s uncompressed content, including every gained and lost name keyed by URL. Raw focused parent/candidate and compact matched reference rows are included.
