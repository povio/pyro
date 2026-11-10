#!/usr/bin/env bash
# All three steps: generate -> train -> use. See each script for its knobs
# (LP_PGO_PORT, LP_PGO_ROUNDS, LP_PGO_EXTRA, LP_PGO_DIR). Extra arguments go to both cargo builds.
set -euo pipefail
here="$(dirname "$0")"
"$here/generate.sh" "$@"
"$here/train.sh"
"$here/use.sh" "$@"
