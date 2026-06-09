#!/usr/bin/env bash
# Initializes the tree-sitter-sequel submodule and generates its parser.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
submodule="$repo_root/crates/tree-sitter-sequel"

git -C "$repo_root" submodule update --init crates/tree-sitter-sequel

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "error: tree-sitter CLI not found. Install it (e.g. 'cargo install tree-sitter-cli')." >&2
  exit 1
fi

echo "Generating tree-sitter-sequel parser..."
(cd "$submodule" && tree-sitter generate)
