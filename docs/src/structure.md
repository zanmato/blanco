# Tables and structure

## In the sidebar

Click a table or view to open its rows in a new tab. Right click it for:

- **Open Structure**, see below
- **Import Data** and **Export Data**
- **Rename**, **Truncate** and **Drop**, each confirmed first

Functions, procedures and triggers open their definition. Table and column
comments are shown where the database has them.

## The structure tab

**Open Structure** shows a table's columns, indexes and foreign keys. Search
narrows the column list. The DDL button shows the table's `CREATE`
statement.

<figure class="screenshot">
<img class="screenshot-light" src="images/structure-latte.webp" alt="The structure of the albums table with a new label column and an index on it marked as pending">
<img class="screenshot-dark" src="images/structure-mocha.webp" alt="">
</figure>

Use **Add Column**, **Add Index** and **Add Foreign Key**, or right click a
row to edit, rename or drop it. Changes are not run straight away:

- Each change is listed as pending, and the lists show the table as it will
  be, with added, changed and dropped rows marked.
- A pending column can be indexed or referenced before anything is applied.
- Undo a change from its row. Changes that depend on it are undone with it.
- **Apply Changes** shows every statement in order. **Confirm** runs them,
  in one transaction where the database supports that. The preview says when
  it doesn't, as on MySQL and ClickHouse.
- **Discard Changes** drops them all.

A change the database can't make is refused when you add it. SQLite, for
example, can't change a column's type, nullability or default, or add a foreign
key to an existing table.
