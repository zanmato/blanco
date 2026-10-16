# Editing data

A result can be edited in place when it comes from one table with a primary
key. A lock icon marks results that can't be edited.

<figure class="screenshot">
<img class="screenshot-light" src="images/inline-edit-latte.webp" alt="A results grid with two edited cells highlighted and a third open for editing">
<img class="screenshot-dark" src="images/inline-edit-mocha.webp" alt="">
</figure>

- Press <kbd>Enter</kbd> or <kbd>F2</kbd> to edit a cell, or double click it.
  <kbd>Tab</kbd> moves to the next cell.
- Right click a cell to **Set NULL**, to add, duplicate or delete rows, and
  on a new row to **Set DEFAULT**.
- Date and time cells have a picker.

Changed cells, new rows and deleted rows are highlighted until you apply them.

## Larger values

While you edit a cell, its expand button opens the value in a bigger editor.
Drag the grip in its corner to resize it. A JSON value opens formatted, with **Format** and **Minify**, and
the editor flags invalid JSON as you type.

<figure class="screenshot">
<img class="screenshot-light" src="images/json-edit-latte.webp" alt="A JSON preferences value open in the expanded cell editor, formatted over several lines">
<img class="screenshot-dark" src="images/json-edit-mocha.webp" alt="">
</figure>

## Applying changes

**Apply edits** shows the SQL that will run. **Confirm** runs it all in one
transaction, so either every change is saved or none is. The grid reloads
afterwards.

**Discard edits** throws the pending changes away.

| Action | Shortcut |
| --- | --- |
| Apply edits | <kbd>Super+Shift+C</kbd>, <kbd>Cmd+Shift+C</kbd> on macOS |
| Discard edits | <kbd>Super+Shift+R</kbd>, <kbd>Cmd+Shift+R</kbd> on macOS |

On a connection tagged PROD, the confirm button says so.
