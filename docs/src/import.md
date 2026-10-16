# Import

Right click a table and choose **Import Data** to load a CSV or JSON file
into it. JSON can be an array of objects or one object per line (NDJSON).

<figure class="screenshot">
<img class="screenshot-light" src="images/import-latte.webp" alt="The import dialog with a CSV of new releases mapped onto the albums table">
<img class="screenshot-dark" src="images/import-mocha.webp" alt="">
</figure>

1. Choose the file. Blanco detects the encoding, and you can override it. For
   CSV, set the delimiter and whether the first row is a header.
2. Map the columns. Each table column takes a file column, a fixed value or
   nothing. A file column can be slugified on the way in.
3. Choose what happens when a row conflicts with an existing key: fail, skip
   the row, or update the columns you pick.

The import runs in one transaction. If it fails, nothing is imported.

To load a SQL dump, use **Run SQL File** on the connection or database, or
[Restore](backup-restore.md) for a `pg_dump` or `mysqldump` file.
