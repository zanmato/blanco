# Terminal and agents

## Terminal

<kbd>Ctrl+`</kbd> opens a terminal below the editor (this is <kbd>Ctrl</kbd>
on macOS too). Each tab has its own shell.

<figure class="screenshot">
<img class="screenshot-light" src="images/terminal-latte.webp" alt="A query tab with a terminal below it running a coding agent">
<img class="screenshot-dark" src="images/terminal-mocha.webp" alt="">
</figure>

## Coding agents

Blanco has an MCP server that lets a coding agent such as Claude Code or
opencode look at your databases and work in your tabs. Turn it on under
**MCP Server** in Settings.

The terminal opens in a workspace folder that is already set up for it: it
holds `.mcp.json` and `opencode.json` pointing at the server, and an
`AGENTS.md` that tells the agent how to use it. Start `claude` or `opencode`
there and it is connected to the tab you opened the terminal from.

The agent can:

- list connections, schemas and tables, and read a table's columns and DDL
- read the SQL in your tab, write SQL into it and run it, with the results in
  the grid
- open new query tabs
- run SQL itself and explain queries

Reads run straight away, with a row limit. Writes ask you first, in a dialog
that shows the SQL, the connection and the database. You can allow them for the
rest of the session. Read only connections refuse writes, and connections
tagged PROD, as well as any `DROP` or `TRUNCATE`, always ask.

To use another client, the MCP Server page in Settings copies the
`claude mcp add` command, the server's URL and its token.
