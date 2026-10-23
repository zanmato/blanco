# The editor

## Completion

Suggestions appear as you type. They cover:

- tables, views and the CTEs in the statement
- columns, also after a table alias such as `o.`
- other schemas, type a schema name and `.` to see what's in it
- built in and user defined functions
- keywords, with the ones that fit where you are listed first
- snippet trigger words, see [Snippets](snippets.md)

**Enter** accepts a suggestion. Settings can make that **Tab**, or both.

When you type `(` after a function name, a popup shows its parameters and
highlights the one you are on.

## Formatting

<kbd>Shift+Alt+F</kbd> or the **Format** button formats the statement under
the cursor for the connection's dialect. How keywords, identifiers and
functions are cased, and how joins and CTEs are indented, is set on the
Formatter page in Settings.

<kbd>Shift+Alt+U</kbd> and <kbd>Shift+Alt+L</kbd> (**Upper Case Keywords**
and **Lower Case Keywords** in the command palette) change only the case of
keywords, type names and words such as `NULL` and `TRUE`, and leave the
layout alone. Strings, comments, function names and identifiers, quoted or
not, keep their case. They work on the selection, or on the statement under
the cursor when nothing is selected, and one undo reverts them.

## Linting

Statements are checked as you type, and problems are underlined.

## Multiple cursors

- <kbd>Alt</kbd> click adds a cursor.
- <kbd>Ctrl+Alt+Up</kbd> and <kbd>Ctrl+Alt+Down</kbd> add one on the line
  above or below. On macOS that is <kbd>Cmd+Option+Up</kbd> and
  <kbd>Cmd+Option+Down</kbd>.
- <kbd>Alt+Shift</kbd> drag selects a block.
- <kbd>Esc</kbd> goes back to one cursor.

## Find and replace

<kbd>Ctrl+F</kbd> opens search in the editor. It can also replace.

## View options

From the View menu, Settings or the command palette:

- **Word Wrap** wraps long lines.
- **Render Whitespace** shows spaces and tabs.
- **Highlight Invisible Characters** boxes characters that look like nothing
  or like a normal space, such as a pasted no break space. It is on by
  default.
- Folding arrows in the gutter fold a statement or a block.
- <kbd>Ctrl+=</kbd>, <kbd>Ctrl+-</kbd> and <kbd>Ctrl+0</kbd> change the
  font size.
