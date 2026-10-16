# Settings and themes

Open Settings from the menu or with <kbd>Super+,</kbd> (<kbd>Cmd+,</kbd> on
macOS). The search field at the top finds a setting on any page.

<figure class="screenshot">
<img class="screenshot-light" src="images/settings-latte.webp" alt="The settings tab">
<img class="screenshot-dark" src="images/settings-mocha.webp" alt="">
</figure>

| Page | What's there |
| --- | --- |
| General | Update checks, and whether the sidebar follows the active tab |
| Editor | Word wrap, whitespace, invisible characters, folding, tabs and tab size, font size, the key that accepts a completion |
| Formatter | Keyword and identifier case, indentation of joins and CTEs, line length |
| Database | Connect, query and script timeouts, idle disconnect, history size, CSV options for copying, row numbers, deferred large values, invisible characters in cells, paths to the backup tools |
| Results | How dates, times and time zones are shown |
| Appearance | Theme, following the system appearance, fonts |
| Environments | Environment tags, their colours and whether they confirm writes |
| MCP Server | Turning the server on, its port, write confirmation, client setup |
| Keybindings | Every shortcut, see [Keyboard shortcuts](shortcuts.md) |

Each page can be reset to its defaults. Settings are stored in
`settings.json` in Blanco's config folder, `~/.config/blanco` on Linux.

## Themes

Blanco comes with the four Catppuccin themes: Latte, Frappé, Macchiato and
Mocha. Pick one on the Appearance page or by typing "Theme" in the command
palette.

Turn on **Follow System Appearance** to switch between a light and a dark theme
of your choice along with the system.
