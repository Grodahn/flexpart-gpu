#!/usr/bin/env bash
set -euo pipefail

# Exercise the build helper in isolation so a failed image build can never be
# hidden by a later command that happens to use an older tagged image.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FUNCTION_SOURCE="$(sed -n '/^oracle_build_pinned()/,/^}/p' "${SCRIPT_DIR}/run-corpus.sh")"
eval "${FUNCTION_SOURCE}"
RESTORE_SOURCE="$(sed -n '/^restore_oracle_checkout()/,/^}/p' "${SCRIPT_DIR}/run-corpus.sh")"
eval "${RESTORE_SOURCE}"
ORACLE_GIT_SOURCE="$(sed -n '/^oracle_git()/,/^}/p' "${SCRIPT_DIR}/run-corpus.sh")"
eval "${ORACLE_GIT_SOURCE}"

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

# A failed Git status command must not be interpreted as a clean checkout.
FLEXPART_DIR="$(mktemp -d)"
trap 'rm -rf "${FLEXPART_DIR}"' EXIT
mkdir -p "${FLEXPART_DIR}/src"
log_error() {
  return 0
}
oracle_git() {
  return 17
}
if restore_oracle_checkout >/dev/null 2>&1; then
  echo "restore_oracle_checkout accepted a failed Git status" >&2
  exit 1
fi

eval "${ORACLE_GIT_SOURCE}"
FLEXPART_DIR="${FLEXPART_DIR}/missing"
if oracle_git rev-parse HEAD >/dev/null 2>&1; then
  echo "oracle_git fell back to the current repository for a missing checkout" >&2
  exit 1
fi
