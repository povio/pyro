# Experimental WebNN CPU graph execution

The feature-enabled LP browser compiled and reused a float32 matmul → broadcast-add → ReLU graph in a Window and a dedicated worker. Across three input writes it produced `[7, 1.5, 0]`, `[2, 3.5, 0]`, and `[0, 0, 6.5]`. All 35 additional small numerical fixtures passed their original WPT expected arrays and tolerance comparator. **The three unmodified WPT pages gained zero named or fully passing pages.**

Source commit `b1f643512d591b50d868ae0ba0393f555d355789` is on the experimental branch and is based on `aa0268b479a6b463da16910b4b743cf38901d962`. The new Cargo feature is off by default. The prototype adds no third-party dependencies or changes to test expectations. [Source](https://github.com/povio/pyro/commit/b1f643512d591b50d868ae0ba0393f555d355789)

## Official matched browser results

A separate build of the experimental source with its feature disabled reproduced every baseline URL, harness status, subtest name and result, including the absence of the interfaces in secure and insecure Window and dedicated-worker checks. [Feature-off WPT results](results/default-topic.json) · [Feature-off exposure](results/exposure-topic-default.json)

The three URL variants were already in the shared non-CSS census selection. The unmodified WPT checkout and manifest were pinned to `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The official runner used wptserve, secure WPT hosts, testdriver, native HTTP/2, manifest-first generated globals, fresh contexts, four workers, and the reference census's no-native-retry runner. All three binaries ran the same URLs.

| Browser | Fully passing pages | Passing named subtests | Other outcomes |
| --- | ---: | ---: | --- |
| LP aa0268b47 | 0/3 | 0 | All harnesses completed. Each reported one failing missing-interface assertion. |
| Experimental LP | 0/3 | 0 | All three setup promises failed. Each reported one outer test as not run. |
| Chrome stable 158.0.8127.37 | 3/3 | 93 | ReLU 18, matmul 23, GEMM 52, including each page’s outer harness test. |

The final official comparison has zero named-pass gains or losses. Its three LP harness statuses changed from OK to Error. The three original missing-interface assertion names disappear and three outer test names become visible. These changes matter even though both LP binaries had zero passes. The broader historical 13,093-name figure was not remeasured by this three-page experiment. [Full page and named pass sets](results/comparison.json) · [Selected URLs](urls.json)

The stable reference was signed Google Chrome 158.0.8127.37 on macOS 27.2.1 arm64 with `--enable-features=WebMachineLearningNeuralNetwork`, ordinary GPU initialization, trusted WPT leaf certificate SPKI and the census host mappings. A preliminary headless diagnostic without that feature flag did not expose WebNN. It is excluded from the table. [Reference identity](results/chrome-identity.json)

### CPU selection

Pinned WebNN IDL declares `accelerated` (default `true`) and `powerPreference`. The pinned WPT helper passes the older `deviceType: 'cpu'` for these three URLs. On a secure WPT origin the exact Chrome executable read the legacy getter, did not read the current `accelerated` getter, and exposed neither a device type nor an accelerated-state attribute. With the recognized legacy CPU request it ran the affine → ReLU graph three times with results identical to the native proof. This confirms the requested mode and numerical output; Chrome did not disclose the physical backend. [Explicit context measurement](results/explicit-cpu.json)

The LP experiment ignores unknown dictionary fields. It accepts an explicit falsy `accelerated` preference and validates `powerPreference`. It rejects accelerator contexts. Consequently the pinned helper's dictionary leaves `accelerated` at its default, and context creation rejects before any official numerical test runs. [Baseline](results/lp-exact.json) · [Experiment](results/feature.json) · [Chrome](results/stable.json)

## What actually executes

The builder enforces context and operand ownership. Compilation visits reachable nodes, validates shape, looks up named bindings, snapshots constants and produces an ordered, reusable execution plan. Contexts serialize writes, dispatch and reads on a promise-microtask timeline. Tensors and graphs have context-bound lifetimes. Matrix multiplication and GEMM call a bounded Rust float32 CPU kernel; broadcast addition, scalar bias indexing and ReLU operate on JavaScript typed arrays.

The six browser controls cover secure Window and dedicated-worker execution, repeated input writes, constant snapshots, BYOB output ranges, dictionary option conversion, wrong shapes and buffer types, graph/tensor ownership, output aliasing, destruction, pending reads, and fresh support limits. A separate origin check found that both secure Window and worker exposed the experimental API, and that neither exposed it on an untrustworthy HTTP origin. [Control outcomes](results/probes-feature.json) · [Origin check](results/exposure-feature.json)

For the supplemental numeric check, the extraction script selects float32 fixtures of the supported ranks from the three unmodified WPT source files, limits each input to 1,024 elements, and excludes large/stress-labelled inputs. A separate harness explicitly requests `accelerated:false` and calls the upstream graph-building and tolerance functions. Actual counts are 27 GEMM, seven ReLU and one matmul. All 35 names also passed in the official Chrome run. These 35 passes and six browser controls are outside the official WPT comparison. [Fixture extraction](source/extract-vectors.cjs) · [Numerical harness](probes/cases/vectors.html)

### Separate operating-system feasibility proof

The included Python script writes an Apple Core ML model with one dense affine layer and one ReLU layer. The Objective-C program compiles the model once, sets `MLComputeUnitsCPUOnly` when loading for prediction, checks input and output float32 descriptors, checks that the input feature rejects a wrong tensor shape and evaluates all three vectors. An independent reviewer regenerated the byte-identical model, rebuilt the executable and reproduced all outputs. Compiler time was one observation per run (27 ms and 19 ms), not a performance benchmark. The experimental browser uses its Rust/JS executor and does not call Core ML. [Model writer](source/model.py) · [Native harness](source/compile_and_run.m) · [Native output](results/coreml-result.json)

## Review and remaining gaps

Independent source review found issues in option conversion and device selection, limits for unsupported operators, shape conversion, float coefficient validation, empty input/output names, duplicate tensor bindings, constant lifetime, and cancellation of outstanding reads on tensor destruction. Those findings were addressed and the five changed source files received a focused recheck. Source review was independent of the recorded feature-enabled LP browser runs. [Source and executable hashes](results/provenance.json)

This is a bounded implementation, not a complete WebNN backend. It limits tensors to 1 MiB of float32, rank 0–5 for elementwise operations, rank two for matrix operands, graphs to 128 reachable nodes, and multiply-accumulate work to 16,777,216 per plan. It has a software timeline on the realm’s event loop, no separate ML task source, per-realm JS branding with remaining WebIDL/prototype differences, no GPU/NPU path, and no production Core ML integration. Only Window and dedicated-worker runs were tested. The pinned minimum table defines 95 operator groups. Even the selected operators require float16 support; matmul requires ranks through five. Support limits report the implemented kernels and empty data-type lists for unavailable kernels. The current slice cannot satisfy the full standard’s minimums. [WebNN specification](https://www.w3.org/TR/webnn/) · [Pinned minimum table](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/webnn/resources/required_datatypes_ranks.json)

## Reproduction

Use the source commit above and the matching JSC archive `0e0dd7038a7a-6f4649e395e0`. The local build scheduler supplied the following Cargo settings. Keep each binary before changing features.

```sh
export CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false
export CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true
cargo build --profile wptiter --features render,pyro-webapi/experimental-webnn-cpu
```

For browser measurements, use Bun, Playwright Core 1.65.1, the pinned WPT checkout and its matching manifest. Export `SOURCE_REPO`, an empty `RUNNER_ROOT`, and `CHROME_APP` (the verified reference app). Run `./prepare-runner.sh` to reconstruct the census measurement runner. Set `WPT_CHECKOUT`, `WPT_MANIFEST`, `LP_BASE`, `LP_EXPERIMENT`, `CHROME_LAUNCHER="$RUNNER_ROOT/census/launch-158"`, and `NODE_PATH` to the directory containing Playwright Core. Run `./run.sh`. The reference launcher requires a fresh private profile and the WPT test certificates. [Runner preparation](prepare-runner.sh) · [Matched measurements](run.sh)

For the standalone macOS proof, use the system Core ML and Foundation frameworks.

```sh
python3 source/model.py affine-relu.mlmodel
xcrun clang -O2 -fobjc-arc -mmacosx-version-min=14.0 -framework CoreML -framework Foundation source/compile_and_run.m -o compile_and_run
./compile_and_run affine-relu.mlmodel
```
