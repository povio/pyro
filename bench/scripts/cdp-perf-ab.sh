#!/usr/bin/env bash
# Builds two revisions in release mode and runs one interleaved cdp-perf comparison between them.
#
# Usage: bench/scripts/cdp-perf-ab.sh BASE_REF [HEAD_REF] [-- cdp-perf.mjs options...]
#   BASE_REF   any commit-ish (e.g. origin/main); built in its own worktree and target directory
#   HEAD_REF   commit-ish to compare; omitted = the current working tree, uncommitted changes included
#   options after `--` go to cdp-perf.mjs, which runs under bun (default: --rounds 5)
#
# Each revision gets a target directory keyed by its full SHA under $LP_PERF_WORK (default
# ${TMPDIR:-/tmp}/lp-cdp-perf), so a rebuild of the same SHA is reused and two SHAs never share one: a
# shared target directory has silently produced identical binaries before (bench/results/build-provenance-audit).
# Results go to $LP_PERF_WORK/results/<base>-<head>-<time>/ (run.json, run.md, compare.md); copy a run into
# bench/results/ only when it is worth keeping as evidence.
# Exit status: that of cdp-perf-compare.mjs (1 on a regression or failed scenario), or of the failing step.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
work=${LP_PERF_WORK:-${TMPDIR:-/tmp}/lp-cdp-perf}
mkdir -p "$work"

base_ref=${1:?usage: cdp-perf-ab.sh BASE_REF [HEAD_REF] [-- cdp-perf options]}
shift
head_ref=
if [[ $# -gt 0 && $1 != -- ]]; then
  head_ref=$1
  shift
fi
[[ ${1:-} == -- ]] && shift
perf_args=("$@")
[[ ${#perf_args[@]} -eq 0 ]] && perf_args=(--rounds 5)

# build_sha SHA -> path of the release binary built from a clean worktree of SHA.
build_sha() {
  local sha=$1 src="$work/src-$1" target="$work/target-$1"
  if [[ ! -d $src ]]; then
    git -C "$root" worktree add --detach "$src" "$sha" >&2
  fi
  # Run cargo inside the worktree so its .cargo/config.toml (JIT-entitled linker) applies.
  (cd "$src" && CARGO_TARGET_DIR="$target" cargo build --release -p lightningpanda >&2)
  echo "$target/release/lightningpanda"
}

base_sha=$(git -C "$root" rev-parse --verify "$base_ref^{commit}")
base_bin=$(build_sha "$base_sha")
if [[ -n $head_ref ]]; then
  head_sha=$(git -C "$root" rev-parse --verify "$head_ref^{commit}")
  head_bin=$(build_sha "$head_sha")
  head_name=${head_sha:0:9}
else
  head_sha=$(git -C "$root" rev-parse HEAD)
  head_name="${head_sha:0:9}$([[ -n $(git -C "$root" status --porcelain -- crates Cargo.toml Cargo.lock) ]] && echo -dirty || true)"
  (cd "$root" && cargo build --release -p lightningpanda >&2)
  head_bin="$(cd "$root" && cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/lightningpanda"
fi

if cmp -s "$base_bin" "$head_bin"; then
  echo "cdp-perf-ab: warning: base and head binaries are byte-identical" >&2
fi

out="$work/results/${base_sha:0:9}-${head_name}-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
bun "$root/bench/scripts/cdp-perf.mjs" --bin "base=$base_bin" --bin "head=$head_bin" --out "$out/run.json" "${perf_args[@]}" || true
echo "cdp-perf-ab: results in $out" >&2
node "$root/bench/scripts/cdp-perf-compare.mjs" "$out/run.json" --base base --head head --md "$out/compare.md"
