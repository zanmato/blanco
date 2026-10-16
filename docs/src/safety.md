# Environments and safety

## Environment tags

Give each connection an environment in its form. The tag colours the
connection in the sidebar and its tabs, so it's clear where a query is about
to run.

Blanco comes with LOCAL, DEV, TEST, STAGING and PROD. Add your own on the
**Environments** page in Settings, with a colour and whether it confirms
writes.

## Confirming writes

On PROD, and on any tag set to confirm writes, Blanco asks before running a
statement that changes data or structure. Statements it can't classify count
as writes. Reads run without asking.

## Read only connections

Check **Read-only** in a connection's form to refuse every statement that
modifies data. That applies to queries, edits in the grid, imports, structure
changes and the MCP server.
