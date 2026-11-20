# DelayNode rendering and history

DelayNode reads fractional positions from a bounded history of PCM quanta. The output reflects the layout of the delayed signal, including channel mixing when a read spans different layouts. Feedback paths use the previous quantum and defer current input writes until all readers have consumed it. The context retains a fed node's history across suspend/reconnect without holding an otherwise unreferenced node alive. Existing single-output nodes and their AudioParams use the same graph.

| 346 ordinary Web Audio URLs | Named passes | Fully passing pages | Timeouts | Errors |
| --- | ---: | ---: | ---: | ---: |
| LP cbdeb1193 | 4,146 | 130 | 19 | 15 |
| LP 2c9a98c91 | 4,305 | 147 | 19 | 15 |
| Local Google Chrome stable 158.0.8127.37 | 5,757 | 321 | 2 | 5 |

The candidate gained 163 named passes, lost four and added 17 fully passing pages. All four lost names include the earlier missing-constructor or missing-factory error text; the implemented constructor and invalid-argument assertions pass. The candidate passed every assertion across all eleven DelayNode pages and both tested feedback/delay-clamping pages. There were no restarts or watchdog kills. The common selection excludes two previously recorded Web Audio Chrome hangs. This is a scoped guard.

Direct checks cover integer and fractional stereo delays, feedback with a minimum one-quantum delay, fan-out through a feedback graph, and history after reconnecting the output. Chrome and LP match on short acyclic impulses. Across render quantum boundaries Chrome downmixes some delayed stereo impulses earlier than LP. Chrome delivered a dry sample in the direct zero-delay feedback probe and no previously fed sample on late output connection; LP applied the one-quantum cycle minimum and retained the earlier input. The samples and the suspended channel-layout transition are recorded separately for each browser. The pinned channel-layout WPT also passes on LP. [Web Audio DelayNode specification](https://webaudio.github.io/web-audio-api/#DelayNode)

LP ran both builds using the exact-158 census runner and four jobs; Chrome rows are matched from the same signed-browser reference on WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Tables retain original assertion names and occurrences; column order is control, candidate and exact Chrome 158. Null denotes an unreported name. The HTML/JS engine remains pinned CI JSC 0e0dd7038a7a-6f4649e395e0. [Reference and complete inventory](https://github.com/povio/pyro/blob/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158/README.md)

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests urls.json --bin "lp=$BIN" --out "$OUT"
```
