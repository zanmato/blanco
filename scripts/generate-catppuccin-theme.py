#!/usr/bin/env python3
"""Generate `crates/blanco/assets/themes/catppuccin.json` from the Catppuccin palette.

The hand-maintained file had drifted from the upstream ports in two ways: roles
were assigned to the wrong surface (the app shell used `base`, which upstream
reserves for the editor), and a large share of the keys were spelled the way
Zed's schema spells them rather than gpui-component's, so they parsed into
nothing and silently fell back to gpui-component's default theme.

Role assignments follow the two official ports:
  - https://github.com/catppuccin/vscode   (packages/catppuccin-vsc/src/theme)
  - https://github.com/catppuccin/zed      (zed.tera)
Where those ports have no equivalent token, the value is derived from the
palette with the simplest rule that keeps the flavor consistent.

Run from the repo root:  python3 scripts/generate-catppuccin-theme.py

The sibling gpui-component apps share this theme. They leave the UI font to the
system rather than pinning it, so they take `--no-ui-font`:

    python3 scripts/generate-catppuccin-theme.py --no-ui-font \\
        --output /mnt/dev/src/broquest/crates/broquest/assets/themes/catppuccin.json
"""

from __future__ import annotations

import argparse
import collections
import json
import pathlib

OUTPUT = pathlib.Path("crates/blanco/assets/themes/catppuccin.json")

# The canonical Catppuccin palette. https://catppuccin.com/palette
PALETTE = {
    "Catppuccin Latte": {
        "rosewater": "#dc8a78", "flamingo": "#dd7878", "pink": "#ea76cb",
        "mauve": "#8839ef", "red": "#d20f39", "maroon": "#e64553",
        "peach": "#fe640b", "yellow": "#df8e1d", "green": "#40a02b",
        "teal": "#179299", "sky": "#04a5e5", "sapphire": "#209fb5",
        "blue": "#1e66f5", "lavender": "#7287fd", "text": "#4c4f69",
        "subtext1": "#5c5f77", "subtext0": "#6c6f85", "overlay2": "#7c7f93",
        "overlay1": "#8c8fa1", "overlay0": "#9ca0b0", "surface2": "#acb0be",
        "surface1": "#bcc0cc", "surface0": "#ccd0da", "base": "#eff1f5",
        "mantle": "#e6e9ef", "crust": "#dce0e8",
    },
    "Catppuccin Frappe": {
        "rosewater": "#f2d5cf", "flamingo": "#eebebe", "pink": "#f4b8e4",
        "mauve": "#ca9ee6", "red": "#e78284", "maroon": "#ea999c",
        "peach": "#ef9f76", "yellow": "#e5c890", "green": "#a6d189",
        "teal": "#81c8be", "sky": "#99d1db", "sapphire": "#85c1dc",
        "blue": "#8caaee", "lavender": "#babbf1", "text": "#c6d0f5",
        "subtext1": "#b5bfe2", "subtext0": "#a5adce", "overlay2": "#949cbb",
        "overlay1": "#838ba7", "overlay0": "#737994", "surface2": "#626880",
        "surface1": "#51576d", "surface0": "#414559", "base": "#303446",
        "mantle": "#292c3c", "crust": "#232634",
    },
    "Catppuccin Macchiato": {
        "rosewater": "#f4dbd6", "flamingo": "#f0c6c6", "pink": "#f5bde6",
        "mauve": "#c6a0f6", "red": "#ed8796", "maroon": "#ee99a0",
        "peach": "#f5a97f", "yellow": "#eed49f", "green": "#a6da95",
        "teal": "#8bd5ca", "sky": "#91d7e3", "sapphire": "#7dc4e4",
        "blue": "#8aadf4", "lavender": "#b7bdf8", "text": "#cad3f5",
        "subtext1": "#b8c0e0", "subtext0": "#a5adcb", "overlay2": "#939ab7",
        "overlay1": "#8087a2", "overlay0": "#6e738d", "surface2": "#5b6078",
        "surface1": "#494d64", "surface0": "#363a4f", "base": "#24273a",
        "mantle": "#1e2030", "crust": "#181926",
    },
    "Catppuccin Mocha": {
        "rosewater": "#f5e0dc", "flamingo": "#f2cdcd", "pink": "#f5c2e7",
        "mauve": "#cba6f7", "red": "#f38ba8", "maroon": "#eba0ac",
        "peach": "#fab387", "yellow": "#f9e2af", "green": "#a6e3a1",
        "teal": "#94e2d5", "sky": "#89dceb", "sapphire": "#74c7ec",
        "blue": "#89b4fa", "lavender": "#b4befe", "text": "#cdd6f4",
        "subtext1": "#bac2de", "subtext0": "#a6adc8", "overlay2": "#9399b2",
        "overlay1": "#7f849c", "overlay0": "#6c7086", "surface2": "#585b70",
        "surface1": "#45475a", "surface0": "#313244", "base": "#1e1e2e",
        "mantle": "#181825", "crust": "#11111b",
    },
}

MODES = {"Catppuccin Latte": "light"}
ACCENT = "mauve"

UI_FONT = "Ubuntu Sans"
FONT = {
    "mono_font.family": "Fira Code",
    "mono_font.size": 12,
    "radius": 6,
    "radius.lg": 8,
}


def alpha(color: str, amount: float) -> str:
    """Append an 8-bit alpha channel to a `#rrggbb` value."""
    return f"{color}{round(amount * 255):02x}"


def colors(p: dict[str, str], light: bool) -> dict[str, str]:
    """Map the palette onto gpui-component's `colors` keys.

    Catppuccin's surface convention, which both official ports follow:
      crust  - app chrome (title bar, tab strip, status bar, activity bar)
      mantle - sidebar, panels, popovers, inactive tabs
      base   - the content surface (editor, active tab, data grid)
    """
    accent = p[ACCENT]
    return {
        # Shell and surfaces.
        "background": p["crust"],
        "foreground": p["text"],
        "border": p["surface0"],
        # Matches the shell, as VS Code's window frame does. Inert for now:
        # gpui-component hardcodes the Linux CSD frame to a flat grey and never
        # reads this token, so it only takes effect once `window_border.rs`
        # is changed to use `cx.theme().window_border`.
        "window.border": p["crust"],
        # Both borders match their own background, so the shell reads as one
        # unbroken surface. This is what the VS Code port does by default; its
        # `extraBordersEnabled` option is what draws them, and it is off.
        # Leaving them unset is not the same thing: they fall back to `border`.
        "title_bar.background": p["crust"],
        "title_bar.border": p["crust"],
        "status_bar.background": p["crust"],
        "status_bar.border": p["crust"],
        "tab_bar.background": p["crust"],
        "tab_bar.segmented.background": p["mantle"],
        "tab.background": p["mantle"],
        "tab.foreground": p["overlay0"],
        "tab.active.background": p["base"],
        "tab.active.foreground": accent,
        "popover.background": p["mantle"],
        "popover.foreground": p["text"],
        "accordion.background": p["base"],
        "tiles.background": p["mantle"],
        "group_box.background": p["mantle"],
        "group_box.foreground": p["text"],
        "group_box.title.foreground": p["text"],
        "description_list.label.background": p["surface0"],
        "description_list.label.foreground": p["subtext1"],
        # `overlay` dims the window behind a modal, so it takes the flavor's
        # darkest color. In Latte that is the text, not crust, which is a
        # light grey there and only faded the window. The text sits much
        # further from Latte's surfaces than crust does from the dark ones,
        # so it needs less of it for a comparable dim.
        "overlay": alpha(p["text"], 0.4) if light else alpha(p["crust"], 0.6),
        # Sidebar.
        "sidebar.background": p["mantle"],
        "sidebar.foreground": p["text"],
        "sidebar.border": p["surface0"],
        "sidebar.accent.background": p["surface0"],
        "sidebar.accent.foreground": p["text"],
        "sidebar.primary.background": accent,
        "sidebar.primary.foreground": p["crust"],
        # Accents and interaction.
        "accent.background": p["surface0"],
        "accent.foreground": p["text"],
        "muted.background": p["surface0"],
        "muted.foreground": p["subtext0"],
        "primary.background": accent,
        "primary.foreground": p["crust"],
        "primary.hover.background": accent,
        "primary.active.background": alpha(accent, 0.6),
        "secondary.background": p["surface0"],
        "secondary.foreground": p["text"],
        "secondary.hover.background": p["surface1"],
        "secondary.active.background": p["surface1"],
        "ring": accent,
        "caret": p["rosewater"],
        "selection.background": alpha(accent, 0.4),
        "input.border": p["surface1"],
        "drag.border": accent,
        "drop_target.background": alpha(accent, 0.2),
        "link": p["blue"],
        "link.hover": p["sky"],
        "link.active": p["sky"],
        # Plain buttons. The primary/secondary/danger/success/warning/info
        # variants fall back to the matching semantic tokens below.
        "button.background": p["surface0"],
        "button.foreground": p["text"],
        "button.hover.background": p["surface1"],
        "button.active.background": p["surface2"],
        # Scrollbars.
        "scrollbar.background": alpha(p["crust"], 0.0),
        "scrollbar.thumb.background": alpha(p["surface2"], 0.5),
        "scrollbar.thumb.hover.background": p["overlay0"],
        # Widgets.
        "skeleton.background": p["surface0"],
        "switch.background": p["surface1"],
        "switch.thumb.background": p["base"],
        "slider.background": p["surface1"],
        "slider.thumb.background": accent,
        "progress.bar.background": accent,
        # Lists.
        "list.background": alpha(p["mantle"], 0.0),
        "list.active.background": p["surface0"],
        "list.active.border": accent,
        "list.even.background": alpha(p["surface0"], 0.25),
        "list.head.background": alpha(p["surface0"], 0.2),
        "list.hover.background": alpha(p["surface0"], 0.5),
        # Tables. The results grid is content, so it sits on `base`.
        "table.background": p["base"],
        "table.head.background": p["mantle"],
        "table.head.foreground": p["subtext0"],
        "table.foot.background": p["mantle"],
        "table.foot.foreground": p["subtext0"],
        "table.even.background": alpha(p["surface0"], 0.25),
        "table.hover.background": alpha(p["surface0"], 0.5),
        "table.active.background": alpha(accent, 0.2),
        "table.active.border": accent,
        "table.row.border": p["surface0"],
        # Semantic. Foregrounds sit on the saturated background, so they take
        # `crust` rather than `text`.
        "danger.background": p["red"],
        "danger.hover.background": p["red"],
        "danger.active.background": p["maroon"],
        "danger.foreground": p["crust"],
        "success.background": p["green"],
        "success.hover.background": p["green"],
        "success.active.background": p["teal"],
        "success.foreground": p["crust"],
        "warning.background": p["peach"],
        "warning.hover.background": p["peach"],
        "warning.active.background": p["yellow"],
        "warning.foreground": p["crust"],
        "info.background": p["blue"],
        "info.hover.background": p["blue"],
        "info.active.background": p["sapphire"],
        "info.foreground": p["crust"],
        # ANSI-ish base colors.
        "base.blue": p["blue"],
        "base.blue.light": alpha(p["blue"], 0.4),
        "base.cyan": p["teal"],
        "base.cyan.light": alpha(p["teal"], 0.4),
        "base.green": p["green"],
        "base.green.light": alpha(p["green"], 0.4),
        "base.magenta": p["pink"],
        "base.magenta.light": alpha(p["pink"], 0.4),
        "base.red": p["red"],
        "base.red.light": alpha(p["red"], 0.4),
        "base.yellow": p["yellow"],
        "base.yellow.light": alpha(p["yellow"], 0.4),
        # Charts. Catppuccin has no chart ramp, so this walks the hue circle in
        # the order Zed's port uses for its indent rainbow.
        "chart.1": p["blue"],
        "chart.2": p["green"],
        "chart.3": p["yellow"],
        "chart.4": p["peach"],
        "chart.5": p["mauve"],
        "chart_bullish": p["green"],
        "chart_bearish": p["red"],
    }


# Syntax roles, read off the Zed port's Mocha theme and expressed as palette
# roles so every flavor derives the same way. gpui-component only reads the
# capture names below; anything else falls back to its first dotted segment.
SYNTAX = {
    "attribute": ("peach", {}),
    "boolean": ("peach", {}),
    "comment": ("overlay2", {"font_style": "italic"}),
    # Spelled with an underscore on purpose: gpui-component's `SyntaxColors`
    # field carries no serde rename, so `comment.doc` never deserializes.
    "comment_doc": ("overlay2", {"font_style": "italic"}),
    "constant": ("peach", {}),
    "constructor": ("flamingo", {}),
    "embedded": ("maroon", {}),
    "emphasis": ("maroon", {"font_style": "italic"}),
    "emphasis.strong": ("maroon", {"font_weight": 700}),
    "enum": ("teal", {"font_weight": 700}),
    "function": ("blue", {}),
    "hint": ("overlay1", {"font_style": "italic"}),
    "keyword": ("mauve", {}),
    "label": ("sapphire", {}),
    "link_text": ("lavender", {}),
    "link_uri": ("blue", {"font_style": "italic"}),
    "number": ("peach", {}),
    "operator": ("sky", {}),
    "predictive": ("overlay0", {}),
    # No Zed equivalent; the port colors `keyword.directive` as pink.
    "preproc": ("pink", {}),
    "primary": ("maroon", {}),
    "property": ("blue", {}),
    "punctuation": ("overlay2", {}),
    "punctuation.bracket": ("overlay2", {}),
    "punctuation.delimiter": ("overlay2", {}),
    "punctuation.list_marker": ("teal", {}),
    "punctuation.special": ("pink", {}),
    "string": ("green", {}),
    "string.escape": ("pink", {}),
    "string.regex": ("peach", {}),
    "string.special": ("pink", {}),
    "string.special.symbol": ("flamingo", {}),
    "tag": ("blue", {}),
    "tag.doctype": ("mauve", {}),
    # No Zed equivalent; matches its `text.literal`.
    "text.code.span": ("green", {}),
    "text.literal": ("green", {}),
    "title": ("text", {"font_weight": 800}),
    "type": ("yellow", {}),
    "variable": ("text", {}),
    "variable.special": ("red", {"font_style": "italic"}),
    "variant": ("red", {}),
}


def highlight(p: dict[str, str]) -> dict:
    accent = p[ACCENT]
    syntax = collections.OrderedDict()
    for name, (role, extra) in sorted(SYNTAX.items()):
        style = collections.OrderedDict(color=p[role])
        style.update(extra)
        syntax[name] = style

    return collections.OrderedDict(
        [
            ("editor.background", p["base"]),
            ("editor.foreground", p["text"]),
            ("editor.gutter.background", p["base"]),
            ("editor.active_line.background", alpha(p["text"], 0.07)),
            ("editor.line_number", p["overlay1"]),
            ("editor.active_line_number", accent),
            ("editor.invisible", alpha(p["overlay2"], 0.4)),
            (
                "status",
                collections.OrderedDict(
                    [
                        ("error", p["red"]),
                        ("error.background", alpha(p["red"], 0.15)),
                        ("error.border", alpha(p["red"], 0.4)),
                        ("warning", p["peach"]),
                        ("warning.background", alpha(p["peach"], 0.15)),
                        ("warning.border", alpha(p["peach"], 0.4)),
                        ("info", p["blue"]),
                        ("info.background", alpha(p["blue"], 0.15)),
                        ("info.border", alpha(p["blue"], 0.4)),
                        ("success", p["green"]),
                        ("success.background", alpha(p["green"], 0.15)),
                        ("success.border", alpha(p["green"], 0.4)),
                        ("hint", p["teal"]),
                        ("hint.background", alpha(p["teal"], 0.15)),
                        ("hint.border", alpha(p["teal"], 0.4)),
                    ]
                ),
            ),
            ("syntax", syntax),
        ]
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, default=OUTPUT)
    parser.add_argument(
        "--no-ui-font",
        action="store_true",
        help="omit `font.family` so the app keeps the system UI font",
    )
    args = parser.parse_args()

    themes = []
    for name, palette in PALETTE.items():
        theme = collections.OrderedDict()
        theme["name"] = name
        mode = MODES.get(name, "dark")
        theme["mode"] = mode
        if not args.no_ui_font:
            theme["font.family"] = UI_FONT
        theme.update(FONT)
        theme["colors"] = collections.OrderedDict(sorted(colors(palette, light=mode == "light").items()))
        theme["highlight"] = highlight(palette)
        themes.append(theme)

    document = collections.OrderedDict(
        [
            (
                "$schema",
                "https://github.com/longbridge/gpui-component/raw/refs/heads/main/.theme-schema.json",
            ),
            ("name", "Catppuccin"),
            ("author", "Catppuccin"),
            ("url", "https://github.com/catppuccin/catppuccin"),
            ("themes", themes),
        ]
    )

    with args.output.open("w") as handle:
        json.dump(document, handle, indent=2)
        handle.write("\n")


if __name__ == "__main__":
    main()
