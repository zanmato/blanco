//! Colors for the terminal grid. The host hands in the sixteen ANSI colors plus
//! foreground, background, cursor and selection; everything else the 256-color
//! and true-color escape sequences can name is derived here.

use gpui::{Hsla, Rgba};
use vte::ansi::{Color, NamedColor, Rgb};

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalPalette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the bright
    /// variants of the same eight in the same order.
    pub ansi: [Hsla; 16],
    pub foreground: Hsla,
    pub background: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
}

/// How much dimmed text fades towards the background.
const DIM_FACTOR: f32 = 0.66;

impl Default for TerminalPalette {
    fn default() -> Self {
        let ansi = [
            (0x1e, 0x1e, 0x2e),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xba, 0xc2, 0xde),
            (0x58, 0x5b, 0x70),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xa6, 0xad, 0xc8),
        ]
        .map(|(red, green, blue)| rgb_components_to_hsla(red, green, blue));
        Self {
            ansi,
            foreground: rgb_components_to_hsla(0xcd, 0xd6, 0xf4),
            background: rgb_components_to_hsla(0x1e, 0x1e, 0x2e),
            cursor: rgb_components_to_hsla(0xf5, 0xe0, 0xdc),
            selection: rgb_components_to_hsla(0x58, 0x5b, 0x70).opacity(0.6),
        }
    }
}

impl TerminalPalette {
    /// The color at a slot of the 269-entry table terminals address: the
    /// sixteen ANSI colors, the 6x6x6 cube, the grey ramp, then the special
    /// slots (foreground, background, cursor, dim variants) alacritty appends.
    pub fn indexed(&self, index: usize) -> Hsla {
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let offset = index - 16;
                let level = |component: usize| -> u8 {
                    if component == 0 {
                        0
                    } else {
                        (component * 40 + 55) as u8
                    }
                };
                rgb_components_to_hsla(
                    level(offset / 36),
                    level((offset / 6) % 6),
                    level(offset % 6),
                )
            }
            232..=255 => {
                let grey = ((index - 232) * 10 + 8) as u8;
                rgb_components_to_hsla(grey, grey, grey)
            }
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            259..=266 => self.ansi[index - 259].opacity(DIM_FACTOR),
            267 => self.foreground,
            268 => self.foreground.opacity(DIM_FACTOR),
            _ => self.foreground,
        }
    }

    /// A cell color as the escape sequences describe it.
    pub fn resolve(&self, color: Color) -> Hsla {
        match color {
            Color::Named(named) => self.indexed(named_color_index(named)),
            Color::Indexed(index) => self.indexed(index as usize),
            Color::Spec(rgb) => rgb_to_hsla(rgb),
        }
    }
}

/// `NamedColor` is laid out to match the indexed table (0..=15, then 256..).
pub(crate) fn named_color_index(named: NamedColor) -> usize {
    named as usize
}

pub(crate) fn rgb_components_to_hsla(red: u8, green: u8, blue: u8) -> Hsla {
    Rgba {
        r: red as f32 / 255.0,
        g: green as f32 / 255.0,
        b: blue as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

pub(crate) fn rgb_to_hsla(rgb: Rgb) -> Hsla {
    rgb_components_to_hsla(rgb.r, rgb.g, rgb.b)
}

/// The 8-bit components a program asking for a color (OSC 4/10/11) expects.
/// Alpha is dropped: the wire format has no room for it.
pub(crate) fn hsla_to_rgb(color: Hsla) -> Rgb {
    let rgba = Rgba::from(color);
    Rgb {
        r: (rgba.r * 255.0).round() as u8,
        g: (rgba.g * 255.0).round() as u8,
        b: (rgba.b * 255.0).round() as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_grey_ramp_follow_xterm() {
        let palette = TerminalPalette::default();
        // Index 16 is pure black, 231 pure white, 196 pure red.
        assert_eq!(hsla_to_rgb(palette.indexed(16)), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            hsla_to_rgb(palette.indexed(231)),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(
            hsla_to_rgb(palette.indexed(196)),
            Rgb { r: 255, g: 0, b: 0 }
        );
        assert_eq!(hsla_to_rgb(palette.indexed(232)), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            hsla_to_rgb(palette.indexed(255)),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
    }

    #[test]
    fn named_colors_map_onto_the_table() {
        let palette = TerminalPalette::default();
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Red)),
            palette.ansi[1]
        );
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::BrightWhite)),
            palette.ansi[15]
        );
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Foreground)),
            palette.foreground
        );
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Background)),
            palette.background
        );
    }
}
