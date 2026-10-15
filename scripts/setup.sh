#!/usr/bin/env bash
# Generates the in-tree tree-sitter-redis parser, whose generated sources are
# gitignored. The SQL grammars come from tree-sitter-sql-polyglot, which ships
# its parsers compressed, so they need no generating here.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "error: tree-sitter CLI not found. Install it (e.g. 'cargo install tree-sitter-cli')." >&2
  exit 1
fi

echo "Generating tree-sitter-redis parser..."
(cd "$repo_root/crates/tree-sitter-redis" && tree-sitter generate)
