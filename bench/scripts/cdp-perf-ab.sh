#!/usr/bin/env bash
# Builds two revisions and runs one interleaved cdp-perf comparison between them.
#
# Usage: bench/scripts/cdp-perf-ab.sh [--shipping-profile] [--auto-filter] [--force]
#                                     BASE_REF [HEAD_REF] [-- cdp-perf.mjs options...]
#   BASE_REF            any commit-ish (e.g. origin/main)
#   HEAD_REF            commit-ish to compare; omitted = HEAD, plus uncommitted changes to files the build reads
#   options after `--`  go to cdp-perf.mjs, which runs under bun (default: --rounds 5)
#   --shipping-profile  build with the real release profile (thin LTO, one codegen unit) for final sign-off.
#                       Default: the `perf` profile of bench/scripts/perf-profile.toml, which rebuilds in a
#                       fraction of the time but is not the shipped code (see "A/B comparison" in bench/README.md)
#   --auto-filter       choose the cdp-perf --filter from the changed paths and print it; a change that cannot
#                       reach the binary (docs, tests) skips the run and exits 0
#   --force             run even though cargo builds are running (wall-clock numbers will suffer)
#
# Layout under $PYRO_PERF_WORK (default ${TMPDIR:-/tmp}/pyro-cdp-perf):
#   slot-a/, slot-b/     perf profile: one reused checkout and target directory per side, so rebuilds are incremental
#   src-SHA/, target-SHA/  shipping profile: a checkout and target directory per SHA, never shared
#   bin-PROFILE-SHA/     the finished binary and its build-info.txt; present and matching = the build is skipped
#   results/BASE-HEAD-PROFILE-TIME/  run.json, run.md, compare.md, stages.txt (time and load per stage); copy a
#                        run into bench/results/ only when it is worth keeping as evidence
#
# A shared target directory has silently produced identical binaries before
# (bench/results/build-provenance-audit), so reused slots are checked rather than trusted:
#   - the checkout is at the requested SHA and clean, right before and right after cargo;
#   - every crate whose sources changed since the slot's last build must not be reported fresh by cargo;
#   - the binary is taken from the path cargo reports for this build and must carry allow-jit (macOS);
#   - base and head must differ, by SHA-256 and by __text/__const sections, unless no file the build reads differs.
# A dirty working tree builds in place, in slot-b's target directory: its first build is cold (other paths), later
# ones incremental. Its binary is named by a hash of the uncommitted state, checked before and after cargo.
#
# Before running: refuses while any cargo build is running and lists them with their owners (unless --force), then
# waits for /tmp/pyro-bench.lock, which serializes benches on this machine and is held until exit. On macOS it also
# builds bench/rss-sampler, for the CPU and instructions-retired columns; on a loaded machine trust the latter.
#
# Exit status: that of cdp-perf-compare.mjs (1 on a regression or failed scenario), 2 for usage errors, 3 when a
# check above refused the run, or that of the failing step.
set -euo pipefail

die() {
  echo "cdp-perf-ab: error: $*" >&2
  exit 3
}
note() { echo "cdp-perf-ab: $*" >&2; }
usage() {
  echo "usage: cdp-perf-ab.sh [--shipping-profile] [--auto-filter] [--force] BASE_REF [HEAD_REF] [-- cdp-perf options]" >&2
  exit 2
}

profile=perf
auto_filter=0
force=0
positional=()
while [[ $# -gt 0 ]]; do
  case $1 in
    --shipping-profile) profile=release ;;
    --auto-filter) auto_filter=1 ;;
    --force) force=1 ;;
    --)
      shift
      break
      ;;
    -*) usage ;;
    *) positional+=("$1") ;;
  esac
  shift
done
perf_args=("$@")
[[ ${#positional[@]} -ge 1 && ${#positional[@]} -le 2 ]] || usage
base_ref=${positional[0]}
head_ref=${positional[1]:-}
if [[ $auto_filter == 1 ]] && [[ " ${perf_args[*]-} " == *" --filter "* ]]; then
  echo "cdp-perf-ab: --auto-filter and --filter exclude each other" >&2
  usage
fi
[[ ${#perf_args[@]} -eq 0 ]] && perf_args=(--rounds 5)

root=$(git rev-parse --show-toplevel)
script_dir=$(cd "$(dirname "$0")" && pwd)
profile_config=$script_dir/perf-profile.toml
work=${PYRO_PERF_WORK:-${TMPDIR:-/tmp}/pyro-cdp-perf}
mkdir -p "$work"

# Everything the build reads. Integration tests, benches and Markdown are not compiled into the binary.
input_paths=(crates vendor Cargo.toml Cargo.lock .cargo scripts/macos rust-toolchain rust-toolchain.toml
  ':(exclude,glob)crates/*/tests/**' ':(exclude,glob)crates/*/benches/**' ':(exclude,glob)**/*.md')

# --- locks -------------------------------------------------------------------------------------------------------

bench_lock=/tmp/pyro-bench.lock
bench_lock_held=0
build_lock=

# wait_for_lock DIR WHAT: mkdir is atomic, so whoever creates the directory holds the lock.
wait_for_lock() {
  local waited=0
  until mkdir "$1" 2>/dev/null; do
    if ((waited % 60 == 0)); then
      note "waiting for $2 ($1, held by another run; if its owner died, rmdir it)"
    fi
    sleep 5
    waited=$((waited + 5))
  done
}
release_build_lock() {
  [[ -n $build_lock ]] && rmdir "$build_lock" 2>/dev/null
  build_lock=
}
# shellcheck disable=SC2329 # invoked by the EXIT trap
release_locks() {
  release_build_lock || true
  if [[ $bench_lock_held == 1 ]]; then
    rmdir "$bench_lock" 2>/dev/null || true
    bench_lock_held=0
  fi
}
trap release_locks EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# One line per running cargo build: whose it is (this repository or another project), its directory and command.
# Exact process-name match: a shell or agent wrapper whose command line merely mentions `cargo build`, this script
# included, is not a build. Other subcommands (metadata from an editor) do not load the machine.
running_cargo_builds() {
  local pid args cwd owner common own_common
  own_common=$(cd "$root" && cd "$(git rev-parse --git-common-dir)" && pwd -P)
  for pid in $(pgrep -x cargo || true); do
    args=$(ps -o command= -p "$pid" 2>/dev/null || true)
    if grep -Eq ' (build|b|test|t|check|c|clippy|run|r|bench|rustc|nextest|install)( |$)' <<<"$args"; then
      cwd=$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' || true)
      common=$(cd "${cwd:-/}" 2>/dev/null && cd "$(git rev-parse --git-common-dir 2>/dev/null)" 2>/dev/null && pwd -P || true)
      owner=other-project
      [[ -n $common && $common == "$own_common" ]] && owner=this-repo
      echo "$owner pid $pid in ${cwd:-?}: cargo${args#*cargo}"
    fi
  done
}
# Any build counts, whoever owns it: the CPU contention is the same. --force is the answer on a machine that never
# goes quiet, together with the load warning at the end of the run.
refuse_if_cargo_is_building() {
  local builds
  builds=$(running_cargo_builds)
  [[ -z $builds ]] && return 0
  if [[ $force == 1 ]]; then
    note "--force: running alongside $(wc -l <<<"$builds" | tr -d ' ') cargo build(s), $(grep -c '^this-repo' <<<"$builds" || true) of them from this repository"
    return 0
  fi
  echo "$builds" >&2
  die "cargo builds are running (above, with their owners); wait for them, or pass --force and distrust the numbers"
}

# --- stage log ---------------------------------------------------------------------------------------------------

load1() {
  if [[ -r /proc/loadavg ]]; then
    cut -d' ' -f1 /proc/loadavg
  else
    sysctl -n vm.loadavg | awk '{print $2}'
  fi
}
stage_log=
stage_name=
stage_start=0
stage_load=
stage_begin() {
  stage_name=$1
  stage_start=$SECONDS
  stage_load=$(load1)
}
stage_end() {
  local line
  line=$(printf '%-12s %5ds  load %s -> %s%s' "$stage_name" $((SECONDS - stage_start)) "$stage_load" "$(load1)" "${1:+  ($1)}")
  note "$line"
  stage_log+="$line"$'\n'
}

# --- what changed ------------------------------------------------------------------------------------------------

base_sha=$(git -C "$root" rev-parse --verify "$base_ref^{commit}")
head_dirty=0
if [[ -n $head_ref ]]; then
  head_sha=$(git -C "$root" rev-parse --verify "$head_ref^{commit}")
else
  head_sha=$(git -C "$root" rev-parse HEAD)
  [[ -n $(git -C "$root" status --porcelain -- "${input_paths[@]}") ]] && head_dirty=1
fi

changed_paths() {
  if [[ $head_dirty == 1 ]]; then
    git -C "$root" diff --name-only "$base_sha"
    git -C "$root" ls-files --others --exclude-standard
  else
    git -C "$root" diff --name-only "$base_sha" "$head_sha"
  fi
}
inputs_differ() {
  if [[ $head_dirty == 1 ]]; then
    [[ -n $(git -C "$root" ls-files --others --exclude-standard -- "${input_paths[@]}") ]] ||
      ! git -C "$root" diff --quiet "$base_sha" -- "${input_paths[@]}"
  elif git -C "$root" diff --quiet "$base_sha" "$head_sha" -- "${input_paths[@]}"; then
    return 1
  fi
}
# Hash of the uncommitted state of the files the build reads; names the binary of a dirty working tree.
worktree_state() {
  {
    git -C "$root" status --porcelain -- "${input_paths[@]}"
    git -C "$root" diff --binary HEAD -- "${input_paths[@]}"
    git -C "$root" ls-files -z --others --exclude-standard -- "${input_paths[@]}" |
      (cd "$root" && xargs -0 shasum -a 256)
  } | shasum -a 256 | cut -c1-12
}

# classify_paths: changed paths on stdin -> the scenario groups they can affect, space-separated. No output = none
# can reach the binary.
classify_paths() {
  local path full='rtt throughput runtime dom page target network lp tos pdf' groups=
  while IFS= read -r path; do
    case $path in
      *.md | docs/*) ;;                                      # documentation
      crates/*/tests/* | crates/*/benches/*) ;;              # not compiled into the binary
      crates/pyro-render/* | */screenshot.rs) groups+=" page lp pdf" ;;
      crates/pyro-webapi/* | crates/pyro-js/* | crates/pyro-jsc-sys/*) groups+=" runtime dom lp" ;;
      crates/pyro-net/*) groups+=" network throughput" ;;
      *) groups+=" $full" ;;                                 # the pyro-cdp engine, and anything not known to be narrower
    esac
  done
  echo "$groups"
}
# filter_for GROUPS -> regex for cdp-perf --filter, or nothing when no scenario group is named.
filter_for() {
  local group alternatives=
  for group in rtt throughput runtime dom page target network lp tos pdf; do
    if [[ " $1 " == *" $group "* ]]; then
      alternatives+="${alternatives:+|}$group"
    fi
  done
  [[ -n $alternatives ]] && echo "^($alternatives)\\."
  return 0
}

if [[ $auto_filter == 1 ]]; then
  groups=$(changed_paths | classify_paths)
  filter=$(filter_for "$groups")
  if [[ -z $filter ]]; then
    note "--auto-filter: only documentation or tests changed between ${base_sha:0:9} and ${head_sha:0:9}; nothing to measure"
    exit 0
  fi
  note "--auto-filter: --filter '$filter'"
  if [[ " $groups " == *" pdf "* ]]; then
    note "--auto-filter: reminder: printing or screenshots changed; cdp-perf measures paper-cut printing only, so also run bench/pdf/run.mjs for .pz-page documents"
  fi
  perf_args+=(--filter "$filter")
fi

# --- builds ------------------------------------------------------------------------------------------------------

jit_entitled() {
  [[ $(uname -s) != Darwin ]] && return 0
  local entitlements
  entitlements=$(codesign -d --entitlements - "$1" 2>/dev/null || true)
  [[ $entitlements == *com.apple.security.cs.allow-jit* ]]
}

# The clean checkout at SHA, right now.
verify_checkout() {
  local src=$1 sha=$2 when=$3 actual dirt
  actual=$(git -C "$src" rev-parse HEAD)
  [[ $actual == "$sha" ]] || die "$src is at $actual $when building, expected $sha"
  dirt=$(git -C "$src" status --porcelain)
  [[ -z $dirt ]] || die "$src is not clean $when building:"$'\n'"$dirt"
}

# Cargo decides freshness from mtimes. A crate whose sources changed since the slot's last build must be rebuilt;
# if cargo calls it fresh, the slot's artifacts are about to pass for a commit they were not built from.
verify_rebuilt() {
  local src=$1 prev=$2 sha=$3 json=$4 src_real dir artifacts
  src_real=$(cd "$src" && pwd -P)
  for dir in $(git -C "$src" diff --name-only "$prev" "$sha" -- 'crates/*/src/*' 'vendor/*/src/*' | cut -d/ -f1,2 | sort -u); do
    artifacts=$(grep -F "\"manifest_path\":\"$src_real/$dir/" "$json" | grep -F '"reason":"compiler-artifact"' || true)
    if [[ -n $artifacts && $artifacts != *'"fresh":false'* ]]; then
      die "$dir changed between ${prev:0:9} and ${sha:0:9}, but cargo reused its artifacts as fresh. If no changed file is compiled in, this is a false alarm; otherwise the slot is stale. Either way, delete ${src%/src} and run again."
    fi
  done
}

# run_cargo SRC TARGET JSON: builds in SRC, so that its .cargo/config.toml (JIT-entitled linker) applies.
run_cargo() {
  local src=$1 target=$2 json=$3
  local args=(build --locked -p pyro --message-format=json-render-diagnostics)
  if [[ $profile == perf ]]; then
    (cd "$src" && CARGO_TARGET_DIR="$target" cargo --config "$profile_config" "${args[@]}" --profile perf >"$json")
  else
    (cd "$src" && CARGO_TARGET_DIR="$target" cargo "${args[@]}" --release >"$json")
  fi
}

# Everything besides the source that changes what a build produces. A cached binary must match it.
build_stamp() {
  echo "commit $1"
  echo "profile $profile"
  [[ $profile == perf ]] && shasum -a 256 <"$profile_config"
  rustc -Vv
  echo "RUSTFLAGS=${RUSTFLAGS:-}"
  echo "PYRO_JSC_DIR=${PYRO_JSC_DIR:-}"
}
cached_binary_matches() {
  local bin_dir=$1 stamp=$2
  [[ -x $bin_dir/pyro && -f $bin_dir/build-info.txt && $(cat "$bin_dir/build-info.txt") == "$stamp" ]]
}

# ensure_binary SLOT SHA ID: leaves the checked binary in $work/bin-$profile-$ID and its path in $built_bin.
# ID is SHA, or SHA-dirty-STATE for a build of the working tree in $root.
built_bin=
ensure_binary() {
  local slot=$1 sha=$2 id=$3 bin_dir="$work/bin-$profile-$3" stamp exe json
  built_bin=$bin_dir/pyro
  stamp=$(build_stamp "$id")
  stage_begin "build-$slot"
  if cached_binary_matches "$bin_dir" "$stamp"; then
    stage_end "cached ${id:0:20}"
    return 0
  fi

  local src target last_built="" prev=""
  if [[ $id != "$sha" ]]; then
    src=$root
    target=$work/slot-$slot/target
    mkdir -p "$work/slot-$slot"
  elif [[ $profile == perf ]]; then
    src=$work/slot-$slot/src
    target=$work/slot-$slot/target
    last_built=$work/slot-$slot/last-built
    mkdir -p "$work/slot-$slot"
  else
    src=$work/src-$sha
    target=$work/target-$sha
  fi

  build_lock=$target.lock
  wait_for_lock "$build_lock" "the build directory"
  if cached_binary_matches "$bin_dir" "$stamp"; then # another run finished it while this one waited
    release_build_lock
    stage_end "cached ${id:0:20}"
    return 0
  fi

  if [[ $id != "$sha" ]]; then
    local state_before state_after
    state_before=$(worktree_state)
    [[ $id == "$sha-dirty-$state_before" ]] || die "the working tree changed before building"
  elif [[ ! -e $src/.git ]]; then
    git -C "$root" worktree add --force --detach "$src" "$sha" >&2
  elif [[ -n $last_built ]]; then
    # Rewrites only the files that differ, with a new mtime, which is what lets cargo see them as changed.
    git -C "$src" checkout --quiet --force --detach "$sha"
    git -C "$src" clean -ffdq
  fi
  [[ $id == "$sha" ]] && verify_checkout "$src" "$sha" before

  if [[ -n $last_built && -f $last_built ]]; then
    prev=$(cat "$last_built")
    # Cleared until this build succeeds: after a failed one, crates it did finish are legitimately fresh.
    rm -f "$last_built"
  fi
  json=$target.build.json
  mkdir -p "$target"
  run_cargo "$src" "$target" "$json"

  if [[ $id == "$sha" ]]; then
    verify_checkout "$src" "$sha" after
  else
    state_after=$(worktree_state)
    [[ $state_after == "$state_before" ]] || die "the working tree changed while building"
  fi
  [[ -n $prev && $prev != "$sha" ]] && verify_rebuilt "$src" "$prev" "$sha" "$json"

  exe=$(sed -n 's/.*"executable":"\([^"]*\/pyro\)".*/\1/p' "$json" | tail -n 1)
  [[ -n $exe && -x $exe ]] || die "cargo reported no pyro executable (see $json)"
  jit_entitled "$exe" || die "$exe lacks com.apple.security.cs.allow-jit; JavaScriptCore would run interpreter-only"

  mkdir -p "$bin_dir"
  rm -f "$bin_dir/build-info.txt"
  cp "$exe" "$bin_dir/pyro.tmp"
  mv -f "$bin_dir/pyro.tmp" "$bin_dir/pyro"
  echo "$stamp" >"$bin_dir/build-info.txt"
  [[ -n $last_built ]] && echo "$sha" >"$last_built"
  release_build_lock || true
  stage_end "built ${id:0:20}"
}

refuse_if_cargo_is_building

# cdp-perf reads CPU time and instructions retired through this sampler; without it they are simply missing.
if [[ $(uname -s) == Darwin ]]; then
  (cd "$script_dir/../rss-sampler" && cargo build --release --quiet) || note "warning: no rss-sampler: no CPU or instruction counts"
fi

ensure_binary a "$base_sha" "$base_sha"
base_bin=$built_bin
head_id=$head_sha
head_name=${head_sha:0:9}
if [[ $head_dirty == 1 ]]; then
  head_state=$(worktree_state)
  head_id=$head_sha-dirty-$head_state
  head_name=${head_sha:0:9}-dirty-$head_state
fi
ensure_binary b "$head_sha" "$head_id"
head_bin=$built_bin

# --- the two binaries must really differ -------------------------------------------------------------------------

# Hashes of code and constants. Build history does not reach them (measured: a cold build and an incremental one
# of a SHA match, across slots), but __cstring holds the slot's path through stylo's generated code, so it is left out.
section_hashes() {
  (cd "$(dirname "$1")" && python3 "$script_dir/macho-section-hash.py" pyro) | grep -v __cstring
}
base_hash=$(shasum -a 256 <"$base_bin")
head_hash=$(shasum -a 256 <"$head_bin")
identical=
if [[ $base_hash == "$head_hash" ]]; then
  identical="byte-identical"
elif [[ $(uname -s) == Darwin ]] && [[ $(section_hashes "$base_bin") == "$(section_hashes "$head_bin")" ]]; then
  identical="identical in code and constants (__text, __const)"
fi
if [[ -n $identical ]]; then
  if inputs_differ; then
    die "$base_bin and $head_bin are $identical although files the build reads differ. Either a build reused stale artifacts or the change does not affect the binary, and then there is nothing to compare. To rule out the first, delete $work/bin-$profile-* and $work/slot-* and run again."
  fi
  note "base and head are $identical, and no file the build reads differs: this is an A/A run"
elif ! inputs_differ; then
  note "note: no file the build reads differs, yet the binaries do; the build itself is not reproducible, so read small differences as noise"
fi

# --- the run -----------------------------------------------------------------------------------------------------

refuse_if_cargo_is_building
stage_begin lock-wait
wait_for_lock "$bench_lock" "the benchmark lock"
bench_lock_held=1
stage_end
refuse_if_cargo_is_building # a wait can be long

out="$work/results/${base_sha:0:9}-${head_name}-$profile-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
stage_begin bench
bun "$root/bench/scripts/cdp-perf.mjs" --bin "base=$base_bin" --bin "head=$head_bin" --out "$out/run.json" "${perf_args[@]}" || true
stage_end
bench_load=$stage_load
note "results in $out"

status=0
bun "$root/bench/scripts/cdp-perf-compare.mjs" "$out/run.json" --base base --head head --md "$out/compare.md" || status=$?

printf '%s' "$stage_log" >"$out/stages.txt"
note "profile $profile; time per stage:"
printf '%s' "$stage_log" >&2
if [[ $profile == perf ]]; then
  note "perf profile: compare base and head with each other, not with release numbers; sign off with --shipping-profile"
fi
cpus=$(getconf _NPROCESSORS_ONLN)
if awk -v load="$bench_load" -v cpus="$cpus" 'BEGIN { exit !(load > cpus) }'; then
  note "warning: load average was $bench_load on $cpus CPUs when the run started; do not trust these verdicts"
fi
exit "$status"
