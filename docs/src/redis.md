# Redis

A Redis connection uses the same editor, with Redis commands instead of SQL.
Commands and their subcommands are highlighted and completed.

Keys are listed in the sidebar, the first 1000 by default. A connection's
menu has:

- **Filter Keys…**, which lists only the keys matching a pattern such as
  `user:*`. Redis matches it, so a large keyspace is not sent over. `*`
  matches any text, `?` one character and `[abc]` one of the characters.
- **Show All Keys**, which clears the pattern.
- **Load More Keys**, offered when more keys match than are listed, which
  lists another 1000.

Choose **Inspect Key** to see a key's value. Strings, lists, sets, hashes,
sorted sets and streams are each shown in the grid. A string stored
compressed with gzip, zstd or lz4 is shown decompressed, and the compression
is named above it.

The inspector shows the key's TTL. **Set TTL…** gives it one in seconds, and
**Remove TTL** makes it permanent again.

Each connection names itself on the server with `CLIENT SETNAME`, so it can
be told apart in `CLIENT LIST`. The name is `Blanco` unless the connection
dialog's **Client Name** says otherwise.

SQL features such as formatting, explain, filtering and inline editing don't
apply to Redis.
