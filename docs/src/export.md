# Export

Right click a table and choose **Export Data** to write all its rows to a
file. The format can be CSV, TSV, JSON, SQL `INSERT` statements, Markdown or
HTML.

<figure class="screenshot">
<img class="screenshot-light" src="images/export-latte.webp" alt="The export dialog for the albums table">
<img class="screenshot-dark" src="images/export-mocha.webp" alt="">
</figure>

The export dialog sets the file name and folder, and per format:

- CSV and TSV: delimiter, quote character, when to quote, header row, the
  text written for `NULL`, and what replaces line breaks
- SQL: one `INSERT` per row or one for all rows
- how binary values are written

To export a query result instead, right click a cell and use **Export as**.
It exports the rows currently in the grid, so with a filter applied you get
the filtered rows.

To export several tables or a whole database as SQL, use
[Back up](backup-restore.md).
