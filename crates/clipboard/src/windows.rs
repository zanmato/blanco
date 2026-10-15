//! The Win32 clipboard, written like GPUI's Windows `write_to_clipboard`,
//! with one clipboard format per representation.

use anyhow::Result;
use windows::Win32::{
    Foundation::HANDLE,
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
            SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
};
use windows::core::{HSTRING, Owned};

use crate::{Representation, TEXT_HTML, TEXT_PLAIN, windows_html_format};

pub(crate) fn write(representations: &[Representation]) -> Result<()> {
    let _clipboard = ClipboardGuard::open()?;
    unsafe { EmptyClipboard()? };
    for representation in representations {
        match representation.mime_type.as_str() {
            TEXT_PLAIN => {
                // CF_UNICODETEXT ends at the first NUL, so embedded ones
                // become spaces.
                let text = String::from_utf8_lossy(&representation.bytes).replace('\0', " ");
                let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                set_clipboard_bytes(&wide, CF_UNICODETEXT.0 as u32)?;
            }
            TEXT_HTML => {
                let html = String::from_utf8_lossy(&representation.bytes);
                let mut bytes = windows_html_format(&html).into_bytes();
                bytes.push(0);
                set_clipboard_bytes(&bytes, register_format("HTML Format")?)?;
            }
            other => set_clipboard_bytes(&representation.bytes, register_format(other)?)?,
        }
    }
    Ok(())
}

fn register_format(name: &str) -> Result<u32> {
    let format = unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) };
    anyhow::ensure!(
        format != 0,
        "registering clipboard format {name}: {}",
        std::io::Error::last_os_error()
    );
    Ok(format)
}

fn set_clipboard_bytes<T>(data: &[T], format: u32) -> Result<()> {
    unsafe {
        // Freed on drop unless the clipboard takes it.
        let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(data))?);
        let pointer = GlobalLock(*global);
        anyhow::ensure!(!pointer.is_null(), "GlobalLock returned null");
        std::ptr::copy_nonoverlapping(
            data.as_ptr().cast::<u8>(),
            pointer.cast::<u8>(),
            std::mem::size_of_val(data),
        );
        // GlobalUnlock reports an "error" carrying NO_ERROR once the lock
        // count reaches zero, which is the expected outcome here.
        if let Err(error) = GlobalUnlock(*global)
            && error.code().is_err()
        {
            return Err(error.into());
        }
        SetClipboardData(format, Some(HANDLE(global.0)))?;
        // The clipboard owns the memory now.
        std::mem::forget(global);
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Result<Self> {
        unsafe { OpenClipboard(None)? };
        Ok(Self)
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseClipboard() } {
            tracing::error!("Failed to close the clipboard: {error}");
        }
    }
}
