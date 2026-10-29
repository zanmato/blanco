# Schema graph

Right click a database or schema and choose **View Schema Graph** to see its
tables and how their foreign keys connect them.

<figure class="screenshot">
<img class="screenshot-light" src="images/schema-graph-latte.webp" alt="The schema graph of the demo database with its tables and foreign key lines">
<img class="screenshot-dark" src="images/schema-graph-mocha.webp" alt="">
</figure>

Each table lists its columns and types, with primary and foreign keys marked.
Lines run from a foreign key column to the column it references. Dashed lines
are [virtual foreign keys](structure.md#virtual-foreign-keys). By default
only the lines of the selected table are drawn. Switch to show all of them.

Drag the background to move around and drag a table to move it.

A table's comment shows under its name, and hovering it shows all of it.
Columns with a comment are marked with a dot, and hovering the column
shows its comment. Comments come from PostgreSQL, CockroachDB, MySQL and
MariaDB, SQL Server's `MS_Description`, ClickHouse and DuckDB. SQLite has
none.

## Other schemas

A foreign key into another schema brings the table it references into the
graph, named with its schema and drawn with a dashed border. On MySQL and
MariaDB the same goes for tables in another database. Once tables of more
than one schema show, each table's header is colored by its schema and a
legend in the corner names the colors. A schema keeps its color from one
session to the next. **Other schemas** in the corner hides those tables
again.

## Finding a table

Type in **Find table or column** to find tables and columns by name,
ignoring case. Once you stop typing, the graph moves to the first match and
selects its table, with a found column marked. <kbd>Enter</kbd> moves to the
next match and <kbd>Shift+Enter</kbd> to the previous one, and the box shows
which match you are on. Write `public.users` to find a table in one schema,
or `users.email` to find a column of one table.

## Exporting

**Export SVG…** in the corner saves the graph as an SVG file, for
documentation or a review. The file holds every table where it is now and
every relationship, whichever tables are selected, in the colors of the
current theme. Comments show as tooltips in viewers that support them. To
get a PNG, convert the file with a tool such as Inkscape or
`rsvg-convert`.
