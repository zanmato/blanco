//! The general pasteboard, written like GPUI's macOS `Pasteboard::write`,
//! with one pasteboard type per representation.

use anyhow::Result;
use objc2_app_kit::{NSPasteboard, NSPasteboardType, NSPasteboardTypeHTML, NSPasteboardTypeString};
use objc2_foundation::{NSData, NSString};

use crate::{Representation, TEXT_HTML, TEXT_PLAIN};

pub(crate) fn write(representations: &[Representation]) -> Result<()> {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    for representation in representations {
        let data = NSData::with_bytes(&representation.bytes);
        let written = match representation.mime_type.as_str() {
            // SAFETY: AppKit's pasteboard type constants are immutable
            // statics that live for the whole process.
            TEXT_PLAIN => {
                pasteboard.setData_forType(Some(&data), unsafe { NSPasteboardTypeString })
            }
            TEXT_HTML => pasteboard.setData_forType(Some(&data), unsafe { NSPasteboardTypeHTML }),
            // Other MIME types go by name; a reader that knows the type
            // asks for it by the same name.
            other => {
                let kind: &NSPasteboardType = &NSString::from_str(other);
                pasteboard.setData_forType(Some(&data), kind)
            }
        };
        anyhow::ensure!(
            written,
            "the pasteboard refused {}",
            representation.mime_type
        );
    }
    Ok(())
}
