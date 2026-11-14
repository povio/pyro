#!/bin/sh
# Sets up the merge driver .gitattributes names for bench/wpt/baseline.json and reftest-baseline.json
# (merge=wpt-baseline: bench/wpt/merge-baseline.mjs). Git does not take driver commands from the repository, so run
# this once per clone. It writes the clone's shared config, which every worktree of the clone reads, and a small
# wrapper in the clone's git directory (the config finds it there, so a moved clone keeps working). From a branch that
# predates this script:
#
#   s=$(git show origin/main:scripts/vendor/setup-merge-drivers.sh) && sh -c "$s"
#
# merge=union (README.pyro.md, crates/pyro-render/tests/integration.rs) is built in and needs nothing.
set -eu
cd "$(git rev-parse --show-toplevel)"
wrapper="$(git rev-parse --path-format=absolute --git-common-dir)/pyro-merge-wpt-baseline.sh"
cat > "$wrapper" <<'EOF'
#!/bin/sh
# Written by scripts/vendor/setup-merge-drivers.sh. Git runs it from the top of the worktree being merged:
#   pyro-merge-wpt-baseline.sh BASE OURS THEIRS MARKER_SIZE PATH   (%O %A %B %L %P; the result goes to OURS)
# It runs the worktree's bench/wpt/merge-baseline.mjs, or origin/main's on a checkout from before the driver (the
# first merge of main into an older branch). Without bun or either script, or when the script fails (an exit other
# than 0, merged, or 1, conflicts left), it leaves git's textual merge, with conflict markers.
tmp=$(mktemp -d "${TMPDIR:-/tmp}/merge-baseline.XXXXXX") || exit 2
trap 'rm -rf "$tmp"' EXIT
cp "$2" "$tmp/ours"
script=bench/wpt/merge-baseline.mjs
if [ ! -f "$script" ]; then
  if git show origin/main:bench/wpt/merge-baseline.mjs > "$tmp/merge-baseline.mjs" 2>/dev/null &&
    git show origin/main:bench/wpt/lib.mjs > "$tmp/lib.mjs" 2>/dev/null; then
    script=$tmp/merge-baseline.mjs
  else
    script=
  fi
fi
if [ -n "$script" ] && command -v bun >/dev/null 2>&1; then
  bun "$script" "$@"
  status=$?
  [ "$status" -le 1 ] && exit "$status"
  echo "pyro-merge-wpt-baseline: $script exited with $status; leaving a textual merge of $5" >&2
  cp "$tmp/ours" "$2"
fi
git merge-file --marker-size="$4" -L ours -L base -L theirs "$2" "$1" "$3"
EOF
chmod +x "$wrapper"
git config merge.wpt-baseline.name 'WPT baseline: page by page, keeping the passes of both sides'
# The wrapper is looked up when git runs the driver (from the top of the worktree), not stored as an absolute path.
# shellcheck disable=SC2016
git config merge.wpt-baseline.driver 'sh "$(git rev-parse --path-format=absolute --git-common-dir)/pyro-merge-wpt-baseline.sh" %O %A %B %L %P'
echo "merge.wpt-baseline: $(git config --get merge.wpt-baseline.driver)"
echo "wrapper: $wrapper"
