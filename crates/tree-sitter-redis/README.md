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

The parser (`src/parser.c`) is generated from `grammar.js` and is not committed.
Run the workspace `scripts/setup.sh` (or `tree-sitter generate` here) to produce
it after cloning.

[tree-sitter]: https://tree-sitter.github.io/
