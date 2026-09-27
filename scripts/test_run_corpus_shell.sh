#!/usr/bin/env bash
set -euo pipefail

# Exercise the build helper in isolation so a failed image build can never be
# hidden by a later command that happens to use an older tagged image.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FUNCTION_SOURCE="$(sed -n '/^oracle_build_pinned()/,/^}/p' "${SCRIPT_DIR}/run-corpus.sh")"
eval "${FUNCTION_SOURCE}"

FORTRAN_COMPOSE_FILE="fake-compose.yml"
RUN_CALLED=0
docker() {
  if [[ " $* " == *" build "* ]]; then
    return 42
  fi
  RUN_CALLED=1
  return 0
}

set +e
oracle_build_pinned 0 >/dev/null 2>&1
BUILD_STATUS=$?
set -e

test "${BUILD_STATUS}" -eq 42
test "${RUN_CALLED}" -eq 0
