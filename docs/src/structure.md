# Tables and structure

## In the sidebar

Click a table or view to open its rows in a new tab. Right click it for:

- **Open Structure**, see below
- **Import Data** and **Export Data**
- **Rename**, **Truncate** and **Drop**, each confirmed first

Functions, procedures and triggers open their definition. Table and column
comments are shown where the database has them.

## The structure tab

**Open Structure** shows a table or view one part at a time. The list on the
left switches between:

- **Columns**, with their type, nullability, default, keys and comment.
  Filter narrows the list by name.
- **Indexes**, with their columns, uniqueness, method and condition.
- **Foreign keys**, the ones the database declares and your virtual keys.
- **Table**, the table's comment and what the database records about it:
  row estimate, data and index size, creation time, the last time its data
  changed, and settings such as the engine and collation on MySQL and
  MariaDB.
- **Create statement**, the table's `CREATE` statement with its comments,
  triggers and grants, ready to copy or open in a query tab.

<figure class="screenshot">
<img class="screenshot-light" src="images/structure-latte.webp" alt="The structure of the albums table with a new label column open in the side panel and its changes pending">
<img class="screenshot-dark" src="images/structure-mocha.webp" alt="">
</figure>

### Changing the structure

Select a row to open it in the panel on the right, or add one with
**Column**, **Index** or **Foreign Key**. The type field suggests the
database's type names as you type, and any type can be typed with its length
or precision. On PostgreSQL 15 and later a unique index can treat NULLs as
equal, so only one row may leave its columns empty.

**Save** in the panel stages the change instead of running it:

- The bar at the bottom counts the pending changes, and the lists show the
  table as it will be, with added, changed and dropped rows marked.
- A pending column can be indexed or referenced before anything is applied.
- Undo a change from its row or the panel. Changes that depend on it are
  undone with it.
- **Show SQL** lists every statement in order. **Apply** runs them, in one
  transaction where the database supports that. The bar says when it
  doesn't, as on MySQL, CockroachDB and ClickHouse. On a connection whose
  environment asks for confirmation, Apply shows the statements first.
- **Discard** drops them all.

A change the database can't make is refused when you save it. SQLite, for
example, can't change a column's type, nullability or default, or add a
foreign key to an existing table.

### Column order

On MySQL, MariaDB and ClickHouse a column can go anywhere among the others.
A new column's **Position** puts it first, after a chosen column or at the
end. Drag a column by its number to move it, which stages a move like any
other change. Dragging is off while the list is sorted or filtered, since it
then doesn't show the table's order. The other databases only add columns at
the end and can't move them, so they offer neither.

### Creating a table

**New Table…** on a connection, database, schema or Tables folder opens the
same tab for a table that doesn't exist yet. Name it in the field at the
top. It starts with an `id` column the database numbers itself where it can
(an identity column on PostgreSQL, `AUTO_INCREMENT` on MySQL and MariaDB).

Add columns, indexes and foreign keys as above. A column can be marked as
part of the primary key here, and on every database the columns can be
placed and dragged into the order the table should have. The bar at the bottom shows the `CREATE TABLE`
statement as it grows, and **Create Table** runs it. The tab then shows the
new table like any other. Foreign keys are declared in the `CREATE TABLE`
itself, so SQLite and DuckDB get them too, and a ClickHouse table is ordered
by its primary key.

### Virtual foreign keys

Views and many older tables declare no foreign keys, so nothing tells Blanco
where a column points. **Virtual Key** in the foreign keys list declares one
that lives in Blanco only: pick the referenced table or view and pair the
columns. It is saved at once and never sent to the database.

Virtual keys work like declared ones: the grid follows them to the
referenced row, and the [schema graph](schema-graph.md) draws them as dashed
lines. They belong to their connection, database and schema, and are deleted
with the connection. A database file opened without saving it as a
connection can't have them.
