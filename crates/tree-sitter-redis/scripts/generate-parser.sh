#!/usr/bin/env bash
# Regenerates the Redis parser from grammar.js and commits it the way the
# polyglot SQL grammars are: Brotli-compressed src/parser.c.br and
# src/node-types.json.br, which build.rs inflates. Building Blanco then needs
# neither the tree-sitter CLI nor Node.
#
# The CLI is pinned to the release zanmato/Tree-sitter-sql-polyglot uses, so
# both grammars share a parser ABI. Set TREE_SITTER to use another binary.
set -euo pipefail

grammar_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$grammar_dir"
if [[ -n "${TREE_SITTER:-}" ]]; then
  "$TREE_SITTER" generate
else
  npx --yes tree-sitter-cli@0.26.3 generate
fi

node --input-type=module - <<'EOF'
import { brotliCompressSync, constants } from 'zlib';
import { readFileSync, writeFileSync } from 'fs';

for (const file of ['src/parser.c', 'src/node-types.json']) {
  const input = readFileSync(file);
  const compressed = brotliCompressSync(input, {
    params: {
      [constants.BROTLI_PARAM_QUALITY]: 11,
      [constants.BROTLI_PARAM_LGWIN]: 24,
      [constants.BROTLI_PARAM_SIZE_HINT]: input.length,
    },
  });
  writeFileSync(`${file}.br`, compressed);
  console.log(`${file}: ${input.length} -> ${compressed.length} bytes`);
}
EOF
