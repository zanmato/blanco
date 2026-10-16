# Running queries

Open a query tab with <kbd>Ctrl+T</kbd>, or right click a database or schema in
the sidebar and choose **New Query**. The tab runs against that connection,
database and schema.

## Run

<kbd>Ctrl+Enter</kbd> runs the statement under the cursor. The editor draws a
frame around that statement, so you can see what will run. With text selected
it runs the selection instead.

A selection with several statements gives one result tab per statement. They
run in order and stop at the first error.

<kbd>Ctrl+.</kbd> or the **Stop** button cancels a running query. Queries that
run longer than the query timeout in Settings are cancelled too.

Every run is saved to the [query history](history.md), errors included.

## Parameters

If a statement has placeholders, Blanco asks for their values before it runs.
A name that appears more than once is asked for once. Values are inserted as
you type them, so put quotes around strings yourself. An empty field is sent as
`NULL`, or as an empty string `''` when its **Empty string** box is ticked.

The values you enter are remembered for the tab's next run. **Clear** empties
every field and forgets them.

Which placeholders are recognised depends on the database:

| Database | Placeholders |
| --- | --- |
| PostgreSQL | `$1` |
| MySQL, MariaDB | `?` |
| SQLite | `?`, `?1`, `:name`, `@name`, `$name` |
| DuckDB | `?`, `$1` |
| SQL Server | `@name` |

## Explain

<kbd>Ctrl+E</kbd> runs `EXPLAIN` for the statement under the cursor. For
PostgreSQL, MySQL and SQLite the plan is shown as a tree. Where the database
reports timings, bars show where the time went.

## Timing

The SQL log and the history show how long a query took, split into execution
(until the first row) and fetch (reading the rest).

## Files

Query tabs can be saved as `.sql` files with <kbd>Ctrl+S</kbd> and **File,
Save As**. Right click a connection or database for **Open SQL File** to edit a
file in a tab, or **Run SQL File** to run a whole file, such as a dump,
without opening it.
