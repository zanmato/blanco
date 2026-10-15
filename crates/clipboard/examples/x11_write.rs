//! Writes an HTML table with a text alternative through the X11 path, then
//! keeps serving it for a while so another program can paste it:
//! `cargo run -p blanco-clipboard --example x11_write`, then
//! `wl-paste --list-types` or `xclip -selection clipboard -t TARGETS -o`.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, XlibDisplayHandle,
};

struct X11Display;

impl HasDisplayHandle for X11Display {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let handle = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));
        // SAFETY: the X11 path reads only the handle's kind.
        Ok(unsafe { DisplayHandle::borrow_raw(handle) })
    }
}

fn main() -> anyhow::Result<()> {
    let html = "<table><tr><th>id</th></tr><tr><td>1</td></tr></table>";
    blanco_clipboard::write(
        &X11Display,
        vec![
            blanco_clipboard::Representation::html(html),
            blanco_clipboard::Representation::text("id\n1"),
        ],
    )?;
    std::thread::sleep(std::time::Duration::from_secs(15));
    Ok(())
}
