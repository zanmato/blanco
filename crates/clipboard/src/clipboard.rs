//! Writes one copy to the system clipboard in several formats at once, such
//! as HTML with a plain text alternative. GPUI's `ClipboardItem` carries only
//! text and images, so pasting an HTML table into a mail or a document would
//! otherwise paste its markup.
//!
//! Each platform module is adapted from GPUI's own clipboard for that
//! platform (Apache-2.0, Zed Industries; its X11 part in turn from arboard),
//! cut down to writing and opened up to any MIME type. Remove the crate once
//! GPUI can write these formats itself.

use anyhow::Result;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod wayland;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod x11;

pub const TEXT_PLAIN: &str = "text/plain";
pub const TEXT_HTML: &str = "text/html";

/// The copy in one format, named by its MIME type. Text is UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Representation {
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

impl Representation {
    pub fn new(mime_type: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            mime_type: mime_type.into(),
            bytes: bytes.into(),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new(TEXT_PLAIN, text.into())
    }

    pub fn html(html: impl Into<String>) -> Self {
        Self::new(TEXT_HTML, html.into())
    }
}

/// Prepare the clipboard for the windows on `window`'s display. On Wayland
/// the clipboard can only be taken with the serial of a recent key press or
/// click, so this starts listening for input as soon as a window opens. On
/// the other platforms it does nothing.
pub fn init(window: &impl HasDisplayHandle) -> Result<()> {
    match display(window)? {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        RawDisplayHandle::Wayland(handle) => wayland::init(handle.display).map(|_| ()),
        _ => Ok(()),
    }
}

/// Replace the clipboard with `representations`, offered in the order given,
/// as copied from `window`. On Wayland the copy is made on a thread once an
/// input event newer than the latest press arrives, so it can follow a copy
/// GPUI made with that press. A window without a platform display, as in
/// tests, is an error on every platform, so callers fall back to GPUI's own
/// clipboard there.
pub fn write(window: &impl HasDisplayHandle, representations: Vec<Representation>) -> Result<()> {
    anyhow::ensure!(!representations.is_empty(), "nothing to copy");
    let display = display(window)?;
    #[cfg(target_os = "macos")]
    {
        let _ = display;
        macos::write(&representations)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = display;
        windows::write(&representations)
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        match display {
            RawDisplayHandle::Wayland(handle) => wayland::write(handle.display, representations),
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => x11::write(representations),
            other => anyhow::bail!("no clipboard for display {other:?}"),
        }
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "freebsd"
    )))]
    {
        let _ = (display, representations);
        anyhow::bail!("no clipboard on this platform")
    }
}

fn display(window: &impl HasDisplayHandle) -> Result<RawDisplayHandle> {
    Ok(window
        .display_handle()
        .map_err(|error| anyhow::anyhow!("the window has no display: {error}"))?
        .as_raw())
}

/// Windows' "HTML Format": the fragment wrapped in a document, behind a
/// header giving the byte offsets of the document and the fragment.
#[cfg(any(target_os = "windows", test))]
fn windows_html_format(fragment: &str) -> String {
    const HEADER: &str = "Version:0.9\r\nStartHTML:{start_html}\r\nEndHTML:{end_html}\r\nStartFragment:{start_fragment}\r\nEndFragment:{end_fragment}\r\n";
    const PREFIX: &str = "<html><body>\r\n<!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment-->\r\n</body></html>";
    // Each placeholder becomes ten digits.
    let header_len = HEADER.len()
        - [
            "{start_html}",
            "{end_html}",
            "{start_fragment}",
            "{end_fragment}",
        ]
        .iter()
        .map(|placeholder| placeholder.len() - 10)
        .sum::<usize>();
    let start_html = header_len;
    let start_fragment = start_html + PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + SUFFIX.len();
    let offset = |value: usize| format!("{value:010}");
    let mut output = HEADER
        .replace("{start_html}", &offset(start_html))
        .replace("{end_html}", &offset(end_html))
        .replace("{start_fragment}", &offset(start_fragment))
        .replace("{end_fragment}", &offset(end_fragment));
    output.push_str(PREFIX);
    output.push_str(fragment);
    output.push_str(SUFFIX);
    output
}

#[cfg(test)]
mod tests {
    use super::windows_html_format;

    #[test]
    fn windows_html_offsets_point_at_the_document_and_fragment() {
        let fragment = "<table><tr><td>å</td></tr></table>";
        let output = windows_html_format(fragment);
        let field = |name: &str| -> usize {
            let start = output.find(name).expect(name) + name.len();
            output[start..start + 10].parse().expect("offset")
        };
        assert_eq!(
            &output[field("StartFragment:")..field("EndFragment:")],
            fragment
        );
        assert!(output[field("StartHTML:")..].starts_with("<html>"));
        assert_eq!(field("EndHTML:"), output.len());
    }
}
