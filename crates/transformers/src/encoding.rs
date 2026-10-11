use std::borrow::Cow;

/// The character encoding an export file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    #[default]
    Utf8,
    /// UTF-8 behind a byte order mark, which Excel needs to read a CSV file
    /// as UTF-8.
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

impl TextEncoding {
    pub const ALL: [TextEncoding; 4] = [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf8Bom => "UTF-8 with BOM",
            TextEncoding::Utf16Le => "UTF-16 LE",
            TextEncoding::Utf16Be => "UTF-16 BE",
        }
    }

    /// The bytes a file starts with. UTF-16 always carries a byte order mark
    /// so readers can tell the byte order.
    pub fn preamble(self) -> &'static [u8] {
        match self {
            TextEncoding::Utf8 => &[],
            TextEncoding::Utf8Bom => &[0xEF, 0xBB, 0xBF],
            TextEncoding::Utf16Le => &[0xFF, 0xFE],
            TextEncoding::Utf16Be => &[0xFE, 0xFF],
        }
    }

    /// Encode `text`. Chunks encoded one after another join into the encoding
    /// of their concatenation, so output can be written as it streams.
    pub fn encode(self, text: &str) -> Cow<'_, [u8]> {
        match self {
            TextEncoding::Utf8 | TextEncoding::Utf8Bom => Cow::Borrowed(text.as_bytes()),
            TextEncoding::Utf16Le => {
                Cow::Owned(text.encode_utf16().flat_map(u16::to_le_bytes).collect())
            }
            TextEncoding::Utf16Be => {
                Cow::Owned(text.encode_utf16().flat_map(u16::to_be_bytes).collect())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode() {
        assert_eq!(*TextEncoding::Utf8.encode("å"), *"å".as_bytes());
        assert_eq!(*TextEncoding::Utf16Le.encode("aå"), [0x61, 0, 0xE5, 0]);
        assert_eq!(*TextEncoding::Utf16Be.encode("aå"), [0, 0x61, 0, 0xE5]);
        // A character outside the BMP is a surrogate pair.
        assert_eq!(
            *TextEncoding::Utf16Be.encode("😀"),
            [0xD8, 0x3D, 0xDE, 0x00]
        );
        assert_eq!(TextEncoding::Utf8Bom.preamble(), [0xEF, 0xBB, 0xBF]);
    }
}
