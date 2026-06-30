#!/usr/bin/env bash
# Initializes the tree-sitter-sequel submodule and generates the tree-sitter
# parsers (sequel + redis) whose generated sources are gitignored.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

git -C "$repo_root" submodule update --init crates/tree-sitter-sequel

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "error: tree-sitter CLI not found. Install it (e.g. 'cargo install tree-sitter-cli')." >&2
  exit 1
fi

echo "Generating tree-sitter-sequel parser..."
(cd "$repo_root/crates/tree-sitter-sequel" && tree-sitter generate)

echo "Generating tree-sitter-redis parser..."
(cd "$repo_root/crates/tree-sitter-redis" && tree-sitter generate)
