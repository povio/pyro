# Combined inert-template and Navigator protocol-handler checkpoint

The exact combined candidate bab023c81 preserves the separately reviewed parser changes at 054ffa4c4 and the patch from b58b00b07. The common base is 8e7144b7e. All 32 parser focus pages, 33 parser controls and 20 eligible Navigator directory pages were run on the candidate. Base and individual-feature receipts are reused. All LP executions used the same pinned JSC build. Chrome observations come from the signed stable 158.0.8127.37 census ee24e07f6 on WPT a421f2fea5eb.

| Observation | Named passes | Fully passing pages |
|---|---:|---:|
| Common LP base | 874 | 37 |
| Individual features on their respective cohorts | 1,195 | 41 |
| Combined LP build | 1,195 | 41 |
| Chrome stable | 1,581 | 73 |

The combined build gained 321 Chrome-passing names with zero losses. The 85 URL results and named pass sets matched the corresponding individual feature runs. Parser focus gained 76, parser controls held steady, and the Navigator cohort gained 245. LP had five exclusive named passes and Chrome had 391. Literal-name matching gives seven and 393, respectively. Two tests embed the random WPT server port in a deliberately invalid authority. names.jsonl.gz retains original strings. Five protocol-handler pages had harness errors on all four observation columns; no credit was imputed. Headless handler invocation, WebDriver automatic acceptance, and parser paths outside inert HTML fragment setters remain outside this implementation.

## Reproduction and evidence

Build the combined head with the local build-exact helper, copy target/wptiter/pyro to candidate, and run the following commands from the measurement directory. run.sh records the three local runner invocations. Parser cohorts use two jobs and Navigator uses four to match the original LP runs. The original signed Chrome census used four jobs.

```sh
sh run.sh
python3 report.py
python3 verify.py evidence
```

runtime.json pins source, executable hashes and engine identities. inputs.json and configs.json identify original shard receipts and server configurations. Each named record contains URL, canonical name, occurrence, then base, individual-feature, combined, and Chrome observations. Each observation retains status and original name; null denotes a missing name. Page records give terminal state, harness status, full-pass flag, passing and total named counts, blob and environment. All 85 selections are disjoint. No LP run restarted or triggered the watchdog.
