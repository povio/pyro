# CI orchestration

`WPT` is the automatic build owner for pull requests and main pushes. Its prepare job compares the checked-out
commit with the pull request's base SHA or the push's before SHA. `scripts/ci/plan.mjs` selects native Linux/PGO
checks using the former standalone workflows' path filters. Changes to orchestration itself exercise both paths.

- macOS WPT depends only on macOS JSC. It does not wait for Linux JSC.
- Linux x64 JSC is produced once per run and shared by WPT and the native Linux check.
- Linux arm64 JSC is produced only when native Linux validation is selected. Its build starts independently of x64.
- When PGO is selected, optimized Linux binaries receive the same bundle and client checks as the plain native
  build, and the comparison worker still builds without PGO. WPT independently tests the wptiter browser.
- The macOS PGO check uses its own instrumented JSC and training; those differ from plain JSC and remain separate.
- Standalone Linux, PGO and macOS JSC workflows remain manually dispatchable. They no longer also start on push/PR.
- Release tags use the same platform workflows with stamped manifests. They retain all architecture, bundle,
  installed-package, extracted-archive and uploaded-checksum checks. Tags and main have distinct runs/cache scopes;
  simultaneous cold tag/main runs can still both build JSC. Release browsers cannot reuse unstamped WPT binaries.

WPT's macOS reftests use four duration-balanced shards. Full WPT uses seven shards with four simultaneous by default.
Set the repository variable `WPT_FULL_PARALLEL` to an integer 1..7, or select `parallel` on a manual full run. This is
independent of workers inside each shard. Set `PYRO_CI_CPUS` in each self-hosted runner's environment when multiple
registrations share one physical machine; registrations are not separate CPU/memory capacity. In-progress full runs
are not cancelled by newer pushes, and the existing gate/retry/baseline semantics are unchanged.

Registry/git downloads are cached separately from compiler objects. WPT does not save large Cargo target archives;
sccache covers ordinary Rust compilation, including vendored/path crates. PGO scripts explicitly disable the compiler
wrapper for instrumented/profile-use builds because sccache does not hash the profile file. JSC consumers on macOS
use their workspace artifact instead of deleting a shared home-directory build another runner might be using.

Validation:

```sh
node --test scripts/ci/plan.test.mjs
actionlint -ignore SC2016
```

`CI configuration` runs these on workflow/action/planner changes. The SC2016 exception covers intentional JavaScript
template strings inside single-quoted bun/node snippets; other workflow and embedded-shell findings remain enabled.

Scheduling tradeoff: full WPT follows completion of the consolidated WPT workflow. On JSC/PGO-toolchain changes it
therefore waits for native Linux/PGO validation as well as the subset gates; ordinary source/client changes do not
select those extra builds. This avoids duplicate producers while keeping the whole-suite publication trigger simple.
The increased full-suite parallelism shortens execution after that trigger, but no end-to-end speedup is claimed until
measured in CI. Moving the full-suite trigger earlier would require a separate, commit-pinned artifact-ready handoff.

macOS JSC output/source/build trees and PGO LLVM/source trees are workspace-local. Their changed cache layouts use
new cache keys, so the first run populates them cold. macOS sccache has a unique job socket and is stopped after the
build; one registration cannot borrow another job's credentials or counters.

Release smoke jobs budget artifact transfer separately (15 minutes) from installed-package tests (5 minutes), within
a 25-minute job limit. This prevents a slow package download consuming the entire test budget; it does not claim to
fix transfer throughput. The v0.0.48 attempt spent 9m14s downloading its package and hit the former 10-minute job limit
only 39 seconds into the test step. The release still cannot publish until all installed-package checks pass.
