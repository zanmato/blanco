# Filtering

The filter bar narrows a table's rows on the server. Blanco adds your
conditions to the query's `WHERE` clause and runs it again, so it filters the
whole table, not only the rows already loaded.

Open it with the filter button above the grid or <kbd>Ctrl+Shift+F</kbd>.

<figure class="screenshot">
<img class="screenshot-light" src="images/filter-latte.webp" alt="The filter bar above a result, filtering albums to the LP format">
<img class="screenshot-dark" src="images/filter-mocha.webp" alt="">
</figure>

## Modes

- **Conditions**: pick a column, an operator and a value. **Add Condition**
  adds another, and **Match All** or **Match Any** decides how they combine.
- **WHERE**: type the condition as SQL, with column completion.
- **Any Column**: find text in any text, JSON or UUID column.

## Operators

`=`, `<>`, `<`, `>`, `<=`, `>=`, contains, does not contain, starts with,
ends with, `LIKE`, `NOT LIKE`, `IN`, `NOT IN`, `IS NULL` and `IS NOT NULL`.

Contains, starts with and ends with ignore case. `IN` takes a comma separated
list. The operator you used last becomes the default.

## From a cell

Right click a cell and choose **Filter by Value** to filter the table to rows
with that value.

The copy button in the filter bar copies the SQL it builds. **Clear** removes
the filter.
