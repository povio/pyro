# Navigator protocol handlers

The two Navigator methods apply HTML's scheme, placeholder, URL parsing and same-origin validation. They are exposed on secure windows. The headless browser declines registration and retains no active handlers.

All 20 selected Navigator directory URLs at WPT a421f2fea5eb were measured on base 8e7144b7e and candidate b58b00b07 with CI JSC 0e0dd7038a7a-6f4649e395e0. The signed Chrome stable 158.0.8127.37 reference comes from the completed same-pin census at ee24e07f6. Both LP executions had zero restarts and watchdog kills.

| Runtime | Named passes | Fully passing URLs |
|---|---:|---:|
| LP before | 34 | 5 |
| LP after | 279 | 7 |
| Chrome | 318 | 12 |

The candidate gained all 244 names on protocol.https.html and one historical-interface assertion, with zero lost names. The insecure-context guard retained both passes. With same-server endpoint normalization Chrome had 39 exclusive passes, LP zero. With literal name identity Chrome had 41 and LP two. The difference is the two assertions whose invalid URL inserts %s immediately before the dynamic HTTPS port. report.py uses the original WPT configuration for this substitution and names.jsonl.gz retains the original strings and occurrence numbers for each engine.

Five WebDriver protocol-handler invocation pages returned harness errors with zero named observations on all three runtimes. Handler invocation and WebDriver automatic acceptance remain uncovered. Eighteen tentative extra-scheme assertions remained failing on LP. Other Navigator differences are listed in pages.json and names.jsonl.gz.

## Commands

From the measurement directory, build the candidate with the pinned build-exact helper, then run the matched selection and generate the report.

```sh
(cd src && ../../build-exact "$PWD/../target")
cp target/wptiter/pyro candidate
sh run.sh before "$PWD/../merged-aa0268/integration-52d8b7c/candidate" > before.log 2>&1
sh run.sh after "$PWD/candidate" > after.log 2>&1
python3 report.py
```

The runner and census input locations are in run.sh and report.py. runtime.json identifies each binary. inputs.json and configs.json identify each matched input receipt and server configuration. The named rows contain URL, canonical name, occurrence, base result, candidate result and Chrome result. Each result contains status and original name. A null result records a missing name.
