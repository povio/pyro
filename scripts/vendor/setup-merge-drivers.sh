#!/bin/sh
# Sets up the merge driver .gitattributes names for bench/wpt/baseline.json and reftest-baseline.json
# (merge=wpt-baseline: bench/wpt/merge-baseline.mjs). Git does not take driver commands from the repository, so run
# this once per clone. It writes the clone's shared config, which every worktree of the clone reads, and a small
# wrapper in the clone's git directory. From a branch that predates this script:
#
#   git show origin/main:scripts/vendor/setup-merge-drivers.sh | sh
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
# first merge of main into an older branch), and falls back to git's textual merge without bun or either script.
dir=bench/wpt
if [ ! -f "$dir/merge-baseline.mjs" ]; then
  dir=$(mktemp -d "${TMPDIR:-/tmp}/merge-baseline.XXXXXX")
  trap 'rm -rf "$dir"' EXIT
  git show origin/main:bench/wpt/merge-baseline.mjs > "$dir/merge-baseline.mjs" 2>/dev/null &&
    git show origin/main:bench/wpt/lib.mjs > "$dir/lib.mjs" 2>/dev/null || dir=
fi
if [ -n "$dir" ] && command -v bun >/dev/null 2>&1; then
  bun "$dir/merge-baseline.mjs" "$@"
  exit $?
fi
git merge-file --marker-size="$4" -L ours -L base -L theirs "$2" "$1" "$3"
EOF
chmod +x "$wrapper"
git config merge.wpt-baseline.name 'WPT baseline: page by page, keeping the passes of both sides'
git config merge.wpt-baseline.driver "sh '$wrapper' %O %A %B %L %P"
echo "merge.wpt-baseline: $(git config --get merge.wpt-baseline.driver)"
