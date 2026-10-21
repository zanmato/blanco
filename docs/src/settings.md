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

### Custom themes

Blanco reads theme files from the `themes` folder in its config folder:

| System | Folder |
| --- | --- |
| Linux | `~/.config/blanco/themes` |
| macOS | `~/Library/Application Support/blanco/themes` |
| Windows | `%APPDATA%\blanco\themes` |

**Open Themes Folder** in the command palette creates the folder and opens it.

The easiest way to make a theme is to start from one of
[Blanco's own themes](https://github.com/zanmato/blanco/tree/main/crates/blanco/assets/themes):

1. Download
   [`catppuccin.json`](https://github.com/zanmato/blanco/blob/main/crates/blanco/assets/themes/catppuccin.json)
   and save it in the themes folder under a name of your own, such as
   `my-theme.json`.
2. A file holds a set of themes. Change the set's `name`, keep the themes you
   want to build on and give each of them a new `name`. Its `mode` says whether
   it is a `light` or a `dark` theme.
3. Change the colors. `colors` covers the interface and `highlight` the syntax
   colors of the editor. The `$schema` line at the top lets an editor such as
   VS Code complete and check the keys as you type.
4. Restart Blanco. Your themes are listed on the Appearance page and in the
   command palette next to the built in ones.

A theme with the same name as a built in one replaces it, so you can also
adjust a single Catppuccin flavour by keeping its name.

Keys Blanco doesn't know are ignored. When a file has any, or can't be read at
all, a notification names them when Blanco starts, which usually points to a
misspelt color name.
