#!/usr/bin/env bash
# Local stand-in for CI: formatting, clippy, and the test suite.
# Pass --db to also require the docker-compose databases (BLANCO_RUN_DB_TESTS=1),
# which turns "server unreachable" from a skip into a failure.
#
# Install as a pre-push hook with:
#   ln -sf ../../scripts/check.sh .git/hooks/pre-push
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

require_db=0
for arg in "$@"; do
  case "$arg" in
    --db) require_db=1 ;;
    -h|--help)
      sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) ;; # git passes hook arguments we do not use
  esac
done

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets --no-deps -- -D warnings

echo "==> cargo test"
if [[ "$require_db" == 1 ]]; then
  BLANCO_RUN_DB_TESTS=1 cargo test --workspace
else
  cargo test --workspace
fi

echo "==> all checks passed"
