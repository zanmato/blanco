# Snippets

Snippets are queries and scripts you use often. They live in the
**Snippets** panel of the sidebar.

<figure class="screenshot">
<img class="screenshot-light" src="images/snippets-latte.webp" alt="The snippets panel with snippets in groups, one open in the snippet editor">
<img class="screenshot-dark" src="images/snippets-mocha.webp" alt="">
</figure>

- **New Snippet** (File menu or the panel) adds a snippet and opens it in the
  snippet editor. Type its name in the panel and press <kbd>Enter</kbd>. In the
  editor, choose whether it is a query or a script, and save it.
- **New Group** adds a folder, named the same way. Right clicking a group adds
  the new snippet or group inside it. Drag snippets and groups to rearrange
  them.
- Double click a snippet to open it in a tab.
- <kbd>F2</kbd> renames the selected snippet or group, or the snippet whose
  editor you are in. <kbd>Enter</kbd> or clicking elsewhere keeps the new name,
  and <kbd>Esc</kbd> leaves it as it was.
- <kbd>Ctrl+D</kbd> (<kbd>Cmd+D</kbd> on macOS) duplicates the selected
  snippet. The copy has no trigger word.
- Right click for **Rename**, **Duplicate**, **Copy Snippet**, **Delete** and
  **Delete Group**.

## Trigger words

A snippet can have a trigger word. Type the word in the editor and it shows up
in completion. Accepting it inserts the snippet. Snippets can contain tab stops
(`$1`, `$2`, and `$0` for the final cursor position), and
<kbd>Tab</kbd> moves between them.
