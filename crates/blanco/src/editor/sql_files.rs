//! `.sql` files in query tabs: open (dialog or drop), save, save as, and the
//! unsaved-changes guard when a file tab, or a structure tab with changes
//! not yet applied, closes.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{App, Context, Entity, ParentElement as _, Window};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogClose, DialogFooter},
    input::EditorState,
    notification::NotificationType,
};

use super::table_structure::TableStructureTab;
use super::tabs::{TabFile, TabType, file_name};
use super::{EditorPanel, TabCreationParams};
use crate::result_ext::ResultExt as _;
use blanco_core::ConnectionContext;

/// Files larger than this open with a pointer to Run SQL File instead, which
/// streams them without loading the editor.
pub const MAX_EDITOR_FILE_BYTES: u64 = 20 * 1024 * 1024;

fn is_gzip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gz"))
}

/// Read a script, decompressing it when it is gzip (by extension or by its
/// magic bytes).
pub fn read_sql_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let bytes = if is_gzip(path) || bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice()).read_to_end(&mut decoded)?;
        decoded
    } else {
        bytes
    };
    String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the file is not UTF-8 text",
        )
    })
}

/// Write a script, gzip compressed when the path ends in `.gz`.
pub fn write_sql_file(path: &Path, text: &str) -> std::io::Result<()> {
    if is_gzip(path) {
        let file = std::fs::File::create(path)?;
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(text.as_bytes())?;
        encoder.finish()?.sync_all()
    } else {
        std::fs::write(path, text)
    }
}

impl EditorPanel {
    fn notify_error(message: String, window: &mut Window, cx: &mut App) {
        window.push_notification((NotificationType::Error, message), cx);
    }

    /// Ask for `.sql` files and open them on `context`'s database.
    pub fn prompt_open_sql_files(
        &mut self,
        context: ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(paths) = paths.await.ok().and_then(Result::ok).flatten() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.open_sql_files(paths, context, window, cx)
            })
            .log_err();
        })
        .detach();
    }

    /// Open files dropped on the window. A drop names no database, so they
    /// open on the active tab's connection.
    pub fn open_dropped_sql_files(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self
            .tabs
            .get(self.active_tab_ix)
            .and_then(TabType::connection_tab)
            .or_else(|| self.tabs.iter().find_map(TabType::connection_tab))
            .map(|tab| tab.context.clone())
        else {
            Self::notify_error(
                "Open a query tab on a connection first: a dropped SQL file opens on the active tab's connection".to_string(),
                window,
                cx,
            );
            return;
        };
        self.open_sql_files(paths, context, window, cx);
    }

    /// Open each file in a new query tab on `context`'s database. A file
    /// already open is brought forward rather than read again, so an open
    /// tab's text is never replaced.
    pub fn open_sql_files(
        &mut self,
        paths: Vec<PathBuf>,
        context: ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            if let Some(ix) = self.tab_for_file(&path) {
                self.activate_tab(ix, window, cx);
                continue;
            }
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            if size > MAX_EDITOR_FILE_BYTES {
                Self::notify_error(
                    format!(
                        "{} is {} MB, too large to edit. Use Run SQL File to run it without opening it.",
                        file_name(&path),
                        size / (1024 * 1024)
                    ),
                    window,
                    cx,
                );
                continue;
            }
            let text = match read_sql_file(&path) {
                Ok(text) => text,
                Err(error) => {
                    Self::notify_error(
                        format!("Could not open {}: {error}", path.display()),
                        window,
                        cx,
                    );
                    continue;
                }
            };
            self.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title: file_name(&path),
                    content: Some(text.clone()),
                    db_id: None,
                    last_run_at: None,
                    context: context.clone(),
                },
                cx,
            );
            let ix = self.tabs.len() - 1;
            if let Some(tab) = self.tabs.get_mut(ix).and_then(TabType::connection_tab_mut) {
                tab.file = Some(TabFile::new(path, Some(text.clone()), &text));
            }
            self.persist_tab(ix, cx);
        }
        cx.notify();
    }

    /// Attach the file a restored tab was saved to, reading it back to know
    /// whether the restored text differs from it.
    pub(super) fn restore_tab_file(&mut self, ix: usize, path: &str, cx: &App) {
        let Some(tab) = self.tabs.get_mut(ix).and_then(TabType::connection_tab_mut) else {
            return;
        };
        let path = PathBuf::from(path);
        let saved = read_sql_file(&path).ok();
        let current = tab.editor.read(cx).text().to_string();
        tab.file = Some(TabFile::new(path, saved, &current));
    }

    fn tab_for_file(&self, path: &Path) -> Option<usize> {
        let canonical = std::fs::canonicalize(path).ok();
        self.tabs.iter().position(|tab| {
            tab.connection_tab()
                .and_then(|tab| tab.file.as_ref())
                .is_some_and(|file| {
                    file.path == path
                        || canonical.is_some()
                            && std::fs::canonicalize(&file.path).ok() == canonical
                })
        })
    }

    /// Recompute the unsaved marker of the tab that owns `editor`.
    pub(super) fn refresh_file_dirty(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .filter_map(TabType::connection_tab_mut)
            .find(|tab| tab.editor == *editor)
        else {
            return;
        };
        let Some(file) = tab.file.as_mut() else {
            return;
        };
        let dirty = !file
            .saved_text
            .as_deref()
            .is_some_and(|saved| rope_equals(editor.read(cx).text(), saved));
        if file.dirty != dirty {
            file.dirty = dirty;
            cx.notify();
        }
    }

    /// Save the active tab to its file, or ask where when it has none.
    pub fn save_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active_tab_ix;
        let path = self
            .tabs
            .get(ix)
            .and_then(TabType::connection_tab)
            .and_then(|tab| tab.file.as_ref())
            .map(|file| file.path.clone());
        match path {
            Some(path) => {
                self.write_tab_to(ix, path, window, cx);
            }
            None => self.save_active_tab_as(window, cx),
        }
    }

    /// Ask for a file to save the active tab to.
    pub fn save_active_tab_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active_tab_ix;
        let Some(tab) = self.tabs.get(ix).and_then(TabType::connection_tab) else {
            return;
        };
        let (directory, suggested) = match &tab.file {
            Some(file) => (file.path.parent().map(Path::to_path_buf), file.name()),
            None => (None, suggested_file_name(&tab.title)),
        };
        let directory = directory
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = path.await.ok().and_then(Result::ok).flatten() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if let Some(other) = this.tab_for_file(&path).filter(|other| *other != ix) {
                    Self::notify_error(
                        format!(
                            "{} is open in another tab. Close that tab before saving over the file.",
                            file_name(&path)
                        ),
                        window,
                        cx,
                    );
                    this.activate_tab(other, window, cx);
                    return;
                }
                this.write_tab_to(ix, path, window, cx);
            })
            .log_err();
        })
        .detach();
    }

    /// Write the tab at `ix` to `path` and make that its file. Returns whether
    /// it was written.
    fn write_tab_to(
        &mut self,
        ix: usize,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(ix).and_then(TabType::connection_tab_mut) else {
            return false;
        };
        let text = tab.editor.read(cx).text().to_string();
        if let Err(error) = write_sql_file(&path, &text) {
            Self::notify_error(
                format!("Could not save {}: {error}", path.display()),
                window,
                cx,
            );
            return false;
        }
        tab.title = file_name(&path);
        tab.file = Some(TabFile::new(path, Some(text.clone()), &text));
        self.persist_tab(ix, cx);
        cx.notify();
        true
    }

    /// Store the tab, its text and file in the app database now rather
    /// than after the next pause in typing.
    fn persist_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(editor) = self
            .tabs
            .get(ix)
            .and_then(TabType::connection_tab)
            .map(|tab| tab.editor.entity_id())
        {
            self.save_tab_for_editor(editor, cx);
        }
    }

    /// Close the tab, asking first when it has unsaved changes to its file
    /// or structure changes not yet applied.
    pub fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::TableStructure(structure)) = self.tabs.get(ix)
            && structure.read(cx).has_pending_changes()
        {
            let structure = structure.clone();
            self.confirm_discard_structure_changes(structure, window, cx);
            return;
        }
        let unsaved = self
            .tabs
            .get(ix)
            .and_then(TabType::connection_tab)
            .and_then(|tab| tab.file.as_ref())
            .filter(|file| file.dirty)
            .map(TabFile::name);
        let Some(name) = unsaved else {
            self.close_tab(ix, cx);
            self.focus_active_tab(window, cx);
            return;
        };
        let weak = cx.entity().downgrade();
        let respond = Rc::new(move |save: bool, window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| {
                if !save || {
                    let path = this
                        .tabs
                        .get(ix)
                        .and_then(TabType::connection_tab)
                        .and_then(|tab| tab.file.as_ref())
                        .map(|file| file.path.clone());
                    path.is_some_and(|path| this.write_tab_to(ix, path, window, cx))
                } {
                    this.close_tab(ix, cx);
                }
            })
            .log_err();
            window.close_dialog(cx);
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let save = respond.clone();
            let discard = respond.clone();
            dialog
                .title(format!("Save changes to {name}?"))
                .child("Your changes are lost if you close the tab without saving.")
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("close-file-cancel").label("Cancel").outline()),
                        )
                        // Sibling `DialogAction`s share a click state, so the
                        // buttons close the dialog themselves.
                        .child(
                            Button::new("close-file-discard")
                                .outline()
                                .label("Don't Save")
                                .on_click(move |_, window, cx| discard(false, window, cx)),
                        )
                        .child(
                            Button::new("close-file-save")
                                .primary()
                                .label("Save")
                                .on_click(move |_, window, cx| save(true, window, cx)),
                        ),
                )
        });
    }
}

impl EditorPanel {
    fn confirm_discard_structure_changes(
        &mut self,
        structure: Entity<TableStructureTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_name = structure.read(cx).table_name.clone();
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let structure = structure.clone();
            dialog
                .title(format!("Discard changes to {table_name}?"))
                .child("The structure changes you have not applied are lost if you close the tab.")
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(
                                Button::new("close-structure-cancel")
                                    .label("Cancel")
                                    .outline(),
                            ),
                        )
                        .child(
                            Button::new("close-structure-discard")
                                .danger()
                                .label("Discard and Close")
                                .on_click(move |_, window, cx| {
                                    // Found again by entity: other tabs may
                                    // have moved while the dialog was open.
                                    weak.update(cx, |this, cx| {
                                        let ix = this.tabs.iter().position(|tab| {
                                            matches!(tab, TabType::TableStructure(tab) if *tab == structure)
                                        });
                                        if let Some(ix) = ix {
                                            this.close_tab(ix, cx);
                                            this.focus_active_tab(window, cx);
                                        }
                                    })
                                    .log_err();
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }
}

/// Whether the rope holds exactly `text`, without copying it. The editor
/// notifies on every cursor move, so the common case of a different length
/// is answered without reading the text.
fn rope_equals(rope: &gpui_component::input::Rope, text: &str) -> bool {
    if rope.len() != text.len() {
        return false;
    }
    let mut rest = text.as_bytes();
    rope.chunks().all(|chunk| {
        let matches = rest.starts_with(chunk.as_bytes());
        rest = &rest[chunk.len().min(rest.len())..];
        matches
    })
}

/// `Query 3` becomes `Query 3.sql`; a title that already ends in `.sql` is
/// kept.
fn suggested_file_name(title: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { "query" } else { title };
    let clean: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    if clean.to_ascii_lowercase().ends_with(".sql") {
        clean
    } else {
        format!("{clean}.sql")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_plain_and_gzip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "SELECT 'åäö';\nSELECT 2;\n";
        for name in ["script.sql", "script.sql.gz"] {
            let path = dir.path().join(name);
            write_sql_file(&path, text).expect("write");
            assert_eq!(read_sql_file(&path).expect("read"), text, "{name}");
        }
        // Gzip content is recognised without the extension too.
        std::fs::copy(
            dir.path().join("script.sql.gz"),
            dir.path().join("dump.sql"),
        )
        .expect("copy");
        assert_eq!(read_sql_file(&dir.path().join("dump.sql")).unwrap(), text);

        std::fs::write(dir.path().join("binary.sql"), [0xff, 0xfe, 0x00]).unwrap();
        let error = read_sql_file(&dir.path().join("binary.sql")).unwrap_err();
        assert_eq!(error.to_string(), "the file is not UTF-8 text");
    }

    #[test]
    fn rope_comparison() {
        let long = "x".repeat(10_000) + "y";
        let rope = gpui_component::input::Rope::from_str(&long);
        assert!(rope_equals(&rope, &long));
        assert!(!rope_equals(&rope, &("x".repeat(10_000) + "z")));
        assert!(!rope_equals(&rope, "x"));
    }

    #[test]
    fn suggested_names() {
        assert_eq!(suggested_file_name("Query 3"), "Query 3.sql");
        assert_eq!(suggested_file_name("report.SQL"), "report.SQL");
        assert_eq!(suggested_file_name("a/b: c"), "a-b- c.sql");
        assert_eq!(suggested_file_name("  "), "query.sql");
    }
}
