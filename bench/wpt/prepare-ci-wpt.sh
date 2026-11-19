#!/bin/sh
set -eu
umask 077

here=$(cd "$(dirname "$0")" && pwd)
mode=${1:?usage: prepare-ci-wpt.sh subset|full}
case "$mode" in subset|full) ;; *) exit 2 ;; esac
runner_work=$(cd "${RUNNER_TEMP:?}/.." && pwd -P)
registration=$(printf '%s\n' "${GITHUB_REPOSITORY:?}" "${RUNNER_NAME:?}" | shasum -a 256)
root="$runner_work/.pyro-wpt/${registration%% *}"
dir="$root/$mode"
stamp="$root/$mode.ready"
rev=$(tr -d '[:space:]' < "$here/wpt-revision.txt")
unset WPT_REVISION WPT_REMOTE

set --
python_identity=
if [ "$mode" = full ]; then
  export WPT_PYTHON="${WPT_PYTHON:-python3}"
  python_identity=$("$WPT_PYTHON" -c 'import os, sys; print(os.path.realpath(sys.executable)); print(sys.version)')
  set -- --all --venv
fi
inputs=$(cd "$here" && shasum -a 256 prepare-ci-wpt.sh fetch-wpt.sh wpt-revision.txt wpt-paths.txt)
key=$(printf '%s\n' "$mode" "$python_identity" "$inputs" | shasum -a 256)
key=${key%% *}

matches_checkout() {
  [ -f "$dir/.git/pyro-fetch-wpt" ] && [ "$(git -C "$dir" rev-parse HEAD)" = "$rev" ] &&
    changes=$(git -C "$dir" status --porcelain --untracked-files=no) && [ -z "$changes" ]
}
venv_ready() {
  [ "$mode" != full ] || {
    cmp -s "$dir/tools/serve/requirements.txt" "$dir/.git/pyro-venv/.pyro-req" &&
      "$dir/.git/pyro-venv/bin/python" -m pip --version >/dev/null
  }
}

if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$key" ] && matches_checkout && venv_ready &&
   git -C "$dir" clean -ffdx -q; then
  echo "Reusing local WPT $mode checkout ($rev)"
else
  echo "Populating local WPT $mode checkout ($rev)"
  mkdir -p "$root"
  rm -f "$stamp" "$stamp.tmp"
  trap 'rm -f "$stamp.tmp"; [ -f "$stamp" ] || rm -rf "$dir"' EXIT
  trap 'exit 1' HUP INT TERM
  rm -rf "$dir"
  "$here/fetch-wpt.sh" "$@" "$dir"
  if ! matches_checkout || ! venv_ready; then
    echo 'WPT checkout did not pass validation' >&2
    exit 1
  fi
  printf '%s\n' "$key" > "$stamp.tmp"
  mv "$stamp.tmp" "$stamp"
fi
printf 'WPT_DIR=%s\n' "$dir" >> "${GITHUB_ENV:?}"
