#!/usr/bin/env bash
# All three steps: generate -> train -> use. See each script for its knobs
# (PYRO_PGO_PORT, PYRO_PGO_ROUNDS, PYRO_PGO_EXTRA, PYRO_PGO_DIR). Extra arguments go to both cargo builds.
set -euo pipefail
here="$(dirname "$0")"
"$here/generate.sh" "$@"
"$here/train.sh"
"$here/use.sh" "$@"
