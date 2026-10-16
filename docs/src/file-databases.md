# SQLite and DuckDB files

SQLite and DuckDB databases are files, and Blanco opens them like documents.

- **File, Open** opens a database file.
- Drop a file on the window to open it.
- Pass files on the command line: `blanco shop.db`.
- Opening a file from your file manager works too.

Blanco tells the two apart by the file's header, so the extension doesn't
matter. An opened file becomes a connection in the sidebar.

**File, New** creates an empty SQLite or DuckDB file and opens it.

## DuckDB

The sidebar lists the file's own database and any you `ATTACH`, each with its
schemas. A DuckDB file can be opened read only, which lets another program
keep writing to it.

Statements in one tab share a session, so `SET`, `USE`, temporary tables and
transactions carry over from one run to the next.
