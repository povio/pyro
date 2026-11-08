#!/usr/bin/env bash
# Step 3 of 3: merge the profiles and build the optimized binary
# (target/release-pgo/lightningpanda, replacing the instrumented one).
#
# llvm-profdata comes from rustup's llvm-tools component (it must match rustc's LLVM;
# the Command Line Tools' copy may not read the .profraw format). Extra arguments go to
# cargo and must match generate.sh's. About 1.5x a clean release build (M1 Max: 9 min).
source "$(dirname "$0")/common.sh"
host="$(rustc -vV | sed -n 's/^host: //p')"
profdata="$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-profdata"
[[ -x "$profdata" ]] || { echo "missing $profdata: rustup component add llvm-tools" >&2; exit 1; }
ls "$PROFRAW_DIR"/*.profraw >/dev/null 2>&1 || { echo "no profiles in $PROFRAW_DIR: run scripts/pgo/train.sh first" >&2; exit 1; }
"$profdata" merge -o "$MERGED" "$PROFRAW_DIR"/*.profraw
"$profdata" show "$MERGED" | sed -n '/Total functions/p;/Total count/p'
RUSTFLAGS="-Cprofile-use=$MERGED $LINKER_FLAG" \
    cargo build --profile release-pgo -p lightningpanda "$@"
echo "optimized: $PGO_BIN"
