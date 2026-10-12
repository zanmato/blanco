//! The viewer a binary cell expands into: a hex dump with an ASCII column,
//! a preview when the bytes are an image, and saving to or loading from a
//! file.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    InteractiveElement, IntoElement, ObjectFit, ParentElement, Render, RenderImage, SharedString,
    Styled, StyledImage, Subscription, Task, UniformListScrollHandle, Window, div, img, px,
    uniform_list,
};
use gpui_component::button::{Button, ButtonGroup, ButtonVariants};
use gpui_component::notification::NotificationType;
use gpui_component::{
    ActiveTheme, Disableable, Selectable, Sizable, WindowExt as _, h_flex, v_flex,
};

use blanco_core::binary::format_size;
use blanco_ui::IconName;

const BYTES_PER_ROW: usize = 16;
const ROW_HEIGHT: f32 = 18.;
/// Larger values are not decoded for a preview: the decoded pixels of an
/// image are many times its file size.
const PREVIEW_LIMIT: usize = 64 * 1024 * 1024;

/// The image formats a value is recognised as, by its leading bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Tiff,
    Ico,
    Svg,
}

impl ImageKind {
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
            Some(Self::Tiff)
        } else if bytes.starts_with(b"\0\0\x01\0") && bytes.len() > 6 {
            Some(Self::Ico)
        } else if bytes.starts_with(b"BM") && bytes.len() > 26 {
            Some(Self::Bmp)
        } else if is_svg(bytes) {
            Some(Self::Svg)
        } else {
            None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG image",
            Self::Jpeg => "JPEG image",
            Self::Gif => "GIF image",
            Self::Webp => "WebP image",
            Self::Bmp => "BMP image",
            Self::Tiff => "TIFF image",
            Self::Ico => "ICO image",
            Self::Svg => "SVG image",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Ico => "ico",
            Self::Svg => "svg",
        }
    }

    fn format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Gif => ImageFormat::Gif,
            Self::Webp => ImageFormat::Webp,
            Self::Bmp => ImageFormat::Bmp,
            Self::Tiff => ImageFormat::Tiff,
            Self::Ico => ImageFormat::Ico,
            Self::Svg => ImageFormat::Svg,
        }
    }
}

/// SVG has no magic number: it is XML whose root element is `svg`, possibly
/// after a byte order mark, an XML declaration, a doctype and comments.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let Ok(text) = std::str::from_utf8(head).or_else(|error| {
        // The cut may have split a character; what came before is still text.
        std::str::from_utf8(&head[..error.valid_up_to()])
    }) else {
        return false;
    };
    let mut rest = text.trim_start();
    loop {
        if rest.starts_with("<svg") {
            return true;
        }
        let end = if rest.starts_with("<?") {
            rest.find("?>").map(|end| end + 2)
        } else if rest.starts_with("<!--") {
            rest.find("-->").map(|end| end + 3)
        } else if rest.starts_with("<!") {
            rest.find('>').map(|end| end + 1)
        } else {
            None
        };
        match end {
            Some(end) => rest = rest[end..].trim_start(),
            None => return false,
        }
    }
}

/// One line of the hex dump: the offset, the bytes in two groups of eight
/// and the same bytes as ASCII with a dot for everything unprintable.
pub fn hex_dump_row(bytes: &[u8], row: usize) -> (String, String, String) {
    let start = row * BYTES_PER_ROW;
    let chunk = bytes
        .get(start..bytes.len().min(start + BYTES_PER_ROW))
        .unwrap_or_default();
    let mut hex = String::with_capacity(BYTES_PER_ROW * 3 + 1);
    for index in 0..BYTES_PER_ROW {
        if index == BYTES_PER_ROW / 2 {
            hex.push(' ');
        }
        match chunk.get(index) {
            Some(byte) => hex.push_str(&format!("{byte:02x} ")),
            None => hex.push_str("   "),
        }
    }
    hex.pop();
    let ascii = chunk
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                byte as char
            } else {
                '.'
            }
        })
        .collect();
    (format!("{start:08x}"), hex, ascii)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    Hex,
    Image,
}

enum Preview {
    /// Nothing decoded, or the value is no image.
    None,
    Decoding,
    Ready(Arc<RenderImage>),
    Failed(SharedString),
}

pub enum BinaryViewerEvent {
    /// A file was loaded: the cell's new bytes.
    Loaded(Arc<Vec<u8>>),
    Close,
}

pub struct BinaryViewer {
    bytes: Arc<Vec<u8>>,
    /// The cell is NULL rather than holding zero bytes.
    is_null: bool,
    kind: Option<ImageKind>,
    mode: ViewMode,
    preview: Preview,
    editable: bool,
    /// File name offered when saving, without an extension.
    file_stem: String,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    decode_task: Option<Task<()>>,
    _release: Subscription,
}

impl EventEmitter<BinaryViewerEvent> for BinaryViewer {}

impl BinaryViewer {
    /// A viewer on `bytes`, `None` for a NULL cell.
    pub fn new(
        bytes: Option<Vec<u8>>,
        editable: bool,
        file_stem: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let is_null = bytes.is_none();
        let mut viewer = Self::empty(editable, file_stem, cx);
        viewer.is_null = is_null;
        viewer.set_bytes(Arc::new(bytes.unwrap_or_default()), cx);
        viewer
    }

    fn empty(editable: bool, file_stem: String, cx: &mut Context<Self>) -> Self {
        Self {
            bytes: Arc::new(Vec::new()),
            is_null: false,
            kind: None,
            mode: ViewMode::Hex,
            preview: Preview::None,
            editable,
            file_stem,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            decode_task: None,
            // GPUI keeps a painted image's texture until told otherwise, so
            // the viewer hands its image back when it goes away.
            _release: cx.on_release(|viewer, cx| viewer.release_preview(cx)),
        }
    }

    #[cfg(test)]
    pub fn image_kind(&self) -> Option<ImageKind> {
        self.kind
    }

    #[cfg(test)]
    pub fn has_preview(&self) -> bool {
        matches!(self.preview, Preview::Ready(_))
    }

    fn set_bytes(&mut self, bytes: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        self.release_preview(cx);
        self.kind = ImageKind::detect(&bytes);
        self.bytes = bytes;
        // An image opens as the image, which is what it is looked at for.
        self.mode = if self.kind.is_some() {
            ViewMode::Image
        } else {
            ViewMode::Hex
        };
        self.scroll_handle = UniformListScrollHandle::new();
        self.decode_preview(cx);
        cx.notify();
    }

    fn decode_preview(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.kind else {
            return;
        };
        if self.bytes.len() > PREVIEW_LIMIT {
            self.preview = Preview::Failed(
                format!(
                    "Too large to preview ({}, the limit is {})",
                    format_size(self.bytes.len()),
                    format_size(PREVIEW_LIMIT)
                )
                .into(),
            );
            return;
        }
        self.preview = Preview::Decoding;
        let bytes = Arc::clone(&self.bytes);
        let svg_renderer = cx.svg_renderer();
        let decode = cx.background_spawn(async move {
            Image::from_bytes(kind.format(), bytes.to_vec()).to_image_data(svg_renderer)
        });
        self.decode_task = Some(cx.spawn(async move |viewer, cx| {
            let decoded = decode.await;
            viewer
                .update(cx, |viewer, cx| {
                    viewer.preview = match decoded {
                        Ok(image) => Preview::Ready(image),
                        Err(error) => Preview::Failed(format!("{error:#}").into()),
                    };
                    cx.notify();
                })
                .ok();
        }));
    }

    /// Give the decoded image back: forget it here and drop its texture from
    /// every window. The decode is our own rather than GPUI's image cache,
    /// which would keep the pixels for the life of the app.
    fn release_preview(&mut self, cx: &mut App) {
        self.decode_task = None;
        if let Preview::Ready(image) = std::mem::replace(&mut self.preview, Preview::None) {
            // Deferred to a task: a window being updated right now is out of
            // the app's reach, and its texture would be left behind.
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.drop_image(image, None));
            })
            .detach();
        }
    }

    fn suggested_file_name(&self) -> String {
        let extension = self.kind.map_or("bin", ImageKind::extension);
        format!("{}.{extension}", self.file_stem)
    }

    fn save_to_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let path = cx.prompt_for_new_path(&directory, Some(&self.suggested_file_name()));
        let bytes = Arc::clone(&self.bytes);
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let written = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::write(&path, bytes.as_slice()) }
                })
                .await;
            let notification = match written {
                Ok(()) => (
                    NotificationType::Success,
                    SharedString::from(format!("Saved {}", path.display())),
                ),
                Err(error) => (
                    NotificationType::Error,
                    SharedString::from(format!("Could not save {}: {error}", path.display())),
                ),
            };
            cx.update(|window, cx| window.push_notification(notification, cx))
                .ok();
        })
        .detach();
    }

    fn load_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Load".into()),
        });
        cx.spawn_in(window, async move |viewer, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::read(&path) }
                })
                .await;
            match read {
                Ok(bytes) => {
                    viewer
                        .update(cx, |viewer, cx| {
                            let bytes = Arc::new(bytes);
                            viewer.is_null = false;
                            viewer.set_bytes(Arc::clone(&bytes), cx);
                            cx.emit(BinaryViewerEvent::Loaded(bytes));
                        })
                        .ok();
                }
                Err(error) => {
                    let message =
                        SharedString::from(format!("Could not read {}: {error}", path.display()));
                    cx.update(|window, cx| {
                        window.push_notification((NotificationType::Error, message), cx)
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn description(&self) -> String {
        if self.is_null {
            return "NULL".to_string();
        }
        let size = format_size(self.bytes.len());
        match self.kind {
            Some(kind) => format!("{}, {size}", kind.label()),
            None => size,
        }
    }

    fn render_hex_rows(&self, range: Range<usize>, cx: &App) -> Vec<gpui::Div> {
        range
            .map(|row| {
                let (offset, hex, ascii) = hex_dump_row(&self.bytes, row);
                h_flex()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_4()
                    .whitespace_nowrap()
                    .child(div().text_color(cx.theme().muted_foreground).child(offset))
                    .child(div().child(hex))
                    .child(div().text_color(cx.theme().muted_foreground).child(ascii))
            })
            .collect()
    }

    fn render_message(&self, message: impl Into<SharedString>, cx: &App) -> gpui::AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .font_family(cx.theme().font_family.clone())
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(message.into())
            .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.mode == ViewMode::Image {
            return match &self.preview {
                Preview::Ready(image) => div()
                    .size_full()
                    .p_2()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(Arc::clone(image))
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::ScaleDown),
                    )
                    .into_any_element(),
                Preview::Decoding => self.render_message("Decoding image…", cx),
                Preview::Failed(error) => self.render_message(error.clone(), cx),
                Preview::None => self.render_message("No preview", cx),
            };
        }
        if self.bytes.is_empty() {
            let message = if self.is_null { "NULL" } else { "Empty value" };
            return self.render_message(message, cx);
        }
        uniform_list(
            "binary-viewer-hex",
            self.bytes.len().div_ceil(BYTES_PER_ROW),
            cx.processor(|viewer, range: Range<usize>, _, cx| viewer.render_hex_rows(range, cx)),
        )
        .track_scroll(&self.scroll_handle)
        .size_full()
        .into_any_element()
    }
}

impl Focusable for BinaryViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BinaryViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .font_family(cx.theme().font_family.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.description()),
                    )
                    .when(self.kind.is_some(), |this| {
                        this.child(
                            ButtonGroup::new("binary-viewer-mode")
                                .xsmall()
                                .outline()
                                .child(
                                    Button::new("binary-viewer-image")
                                        .label("Image")
                                        .selected(mode == ViewMode::Image)
                                        .on_click(cx.listener(|viewer, _, _, cx| {
                                            viewer.mode = ViewMode::Image;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("binary-viewer-hex-mode")
                                        .label("Hex")
                                        .selected(mode == ViewMode::Hex)
                                        .on_click(cx.listener(|viewer, _, _, cx| {
                                            viewer.mode = ViewMode::Hex;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(
                        Button::new("binary-viewer-save")
                            .icon(IconName::Save)
                            .label("Save to File")
                            .ghost()
                            .xsmall()
                            .disabled(self.is_null)
                            .on_click(cx.listener(|viewer, _, window, cx| {
                                viewer.save_to_file(window, cx);
                            })),
                    )
                    .when(self.editable, |this| {
                        this.child(
                            Button::new("binary-viewer-load")
                                .icon(IconName::FolderOpen)
                                .label("Load File")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|viewer, _, window, cx| {
                                    viewer.load_from_file(window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("binary-viewer-close")
                            .icon(IconName::Minimize)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(BinaryViewerEvent::Close);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .child(self.render_body(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_recognised_by_their_leading_bytes() {
        assert_eq!(
            ImageKind::detect(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            Some(ImageKind::Png)
        );
        assert_eq!(
            ImageKind::detect(b"\xff\xd8\xff\xe0\0\x10JFIF"),
            Some(ImageKind::Jpeg)
        );
        assert_eq!(
            ImageKind::detect(b"GIF89a\x01\0\x01\0"),
            Some(ImageKind::Gif)
        );
        assert_eq!(
            ImageKind::detect(b"RIFF\x24\0\0\0WEBPVP8 "),
            Some(ImageKind::Webp)
        );
        assert_eq!(ImageKind::detect(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(ImageKind::detect(b"hello"), None);
        assert_eq!(ImageKind::detect(b""), None);
        // Two bytes are not enough to call something a bitmap.
        assert_eq!(ImageKind::detect(b"BM"), None);
    }

    #[test]
    fn svg_is_recognised_through_its_prolog() {
        assert_eq!(
            ImageKind::detect(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some(ImageKind::Svg)
        );
        let with_prolog =
            b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!-- logo -->\n<!DOCTYPE svg>\n<svg/>";
        assert_eq!(ImageKind::detect(with_prolog), Some(ImageKind::Svg));
        assert_eq!(ImageKind::detect(b"<?xml version=\"1.0\"?><html/>"), None);
        assert_eq!(ImageKind::detect(b"<p>not an image <svg/></p>"), None);
    }

    #[test]
    fn hex_dump_rows_line_up() {
        let bytes: Vec<u8> = (0x40..0x40 + 20).collect();
        assert_eq!(
            hex_dump_row(&bytes, 0),
            (
                "00000000".to_string(),
                "40 41 42 43 44 45 46 47  48 49 4a 4b 4c 4d 4e 4f".to_string(),
                "@ABCDEFGHIJKLMNO".to_string()
            )
        );
        // A short last row keeps the width of a full one.
        let (offset, hex, ascii) = hex_dump_row(&bytes, 1);
        assert_eq!(offset, "00000010");
        assert_eq!(hex.len(), hex_dump_row(&bytes, 0).1.len());
        assert!(hex.starts_with("50 51 52 53   "));
        assert_eq!(ascii, "PQRS");
        assert_eq!(hex_dump_row(&[0x00, 0x7f, 0xff, b' '], 0).2, "... ");
    }
}
