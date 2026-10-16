# Process list

Right click a connection and choose **Process List**, or pick it under a
database in the command palette, to see the server's sessions: id, user,
database, state, how long it has been running and the query.

Refresh the list by hand or on an interval. Right click a session to:

- **Cancel Query**: stop the session's current query and keep the session
- **Kill Session**: end the session

Both ask first, warn on a PROD connection and are refused on a read only one.

| Database | Cancel Query | Kill Session |
| --- | --- | --- |
| PostgreSQL | yes | yes |
| MySQL, MariaDB | yes | yes |
| SQL Server | no | yes |
| ClickHouse | yes | no |
