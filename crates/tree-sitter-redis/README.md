# tree-sitter-redis

A [tree-sitter][] grammar for Redis command syntax, used by Blanco to
syntax-highlight the editor when the active connection is a Redis backend.

The grammar classifies each line into a command keyword, an optional subcommand
for container commands (`CONFIG GET`, `CLIENT KILL`, ...), quoted strings,
integers, floats, and bare arguments. The command and subcommand keyword lists
are generated from a live server in `grammar/keywords.js`; regenerate them with:

```bash
python3 scripts/extract-redis-commands.py --host 127.0.0.1 --port 6400 --password blanco
```

The parser is generated from `grammar.js` and committed Brotli-compressed
(`src/parser.c.br`, `src/node-types.json.br`), which `bindings/rust/build.rs`
inflates, so building needs neither the tree-sitter CLI nor Node. After changing
the grammar, regenerate it with `scripts/generate-parser.sh` (needs Node). It
runs tree-sitter-cli 0.26.3, the release the polyglot SQL grammars use.

[tree-sitter]: https://tree-sitter.github.io/
