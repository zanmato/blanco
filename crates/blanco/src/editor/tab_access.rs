//! Tab operations the MCP bridge drives: enumerate, resolve, read, write,
//! create, activate and run. A child module of `editor` so it can reach the
//! panel's private tab list without widening its visibility.

use blanco_core::ConnectionContext;
use gpui::{App, Context, Window};
use gpui_component::input::RopeExt as _;

use super::{EditorPanel, TabCreationParams, TabType, tabs::ConnectionBackedTab};
use crate::mcp::bridge::{
    TabConnection, TabContent, TabEdit, TabSelector, TabSummary, WriteOutcome,
};

/// Maximum lines to return when no range is specified
pub(crate) const READ_TAB_DEFAULT_LIMIT: usize = 200;

/// A line-numbered window of a tab's text. Line numbers are 1-based and
/// inclusive; an empty tab reports zeros.
pub(crate) struct TabSlice {
    pub content: String,
    pub total_lines: usize,
    pub start_line: usize,
    pub end_line: usize,
}

impl TabSlice {
    /// Whether lines after `end_line` were left out.
    pub(crate) fn truncated(&self) -> bool {
        self.total_lines > self.end_line
    }
}

/// Cut `rope` to the requested 1-based inclusive range, numbering each line.
/// Without a range the first `READ_TAB_DEFAULT_LIMIT` lines are returned so a
/// huge buffer does not flood the model; a half-open range extends to the
/// buffer's edge. Out-of-range bounds are clamped rather than rejected.
pub(crate) fn slice_tab_text(
    rope: &ropey::Rope,
    requested_start: Option<usize>,
    requested_end: Option<usize>,
) -> TabSlice {
    let total_lines = rope.lines_len();
    if total_lines == 0 {
        return TabSlice {
            content: String::new(),
            total_lines: 0,
            start_line: 0,
            end_line: 0,
        };
    }

    let (start, end) = match (requested_start, requested_end) {
        (Some(start), Some(end)) => (start, end),
        (Some(start), None) => (start, total_lines),
        (None, Some(end)) => (1, end),
        (None, None) if total_lines > READ_TAB_DEFAULT_LIMIT => (1, READ_TAB_DEFAULT_LIMIT),
        (None, None) => (1, total_lines),
    };

    let start = start.clamp(1, total_lines);
    let end = end.min(total_lines).max(start);

    let slice = rope.slice_lines((start - 1)..end);
    let content = slice
        .to_string()
        .lines()
        .enumerate()
        .map(|(offset, line)| format!("{}: {}", start + offset, line))
        .collect::<Vec<_>>()
        .join("\n");

    TabSlice {
        content,
        total_lines,
        start_line: start,
        end_line: end,
    }
}

/// How `content` is combined with the tab's current text. The MCP `write_tab`
/// tool's JSON parameter deserializes straight into it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WriteOperation {
    #[default]
    ReplaceAll,
    InsertBeforeLine,
    ReplaceLines,
}

impl WriteOperation {
    /// One-line confirmation of what a successful write did.
    pub(crate) fn summary(self, start_line: Option<usize>, end_line: Option<usize>) -> String {
        match self {
            WriteOperation::ReplaceAll => "Content written to tab".to_string(),
            WriteOperation::InsertBeforeLine => {
                format!("Inserted before line {}", start_line.unwrap_or(0))
            }
            WriteOperation::ReplaceLines => format!(
                "Replaced lines {}-{}",
                start_line.unwrap_or(0),
                end_line.unwrap_or(0)
            ),
        }
    }
}

pub(crate) fn apply_line_operation(
    current_text: &str,
    operation: WriteOperation,
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, String> {
    match operation {
        WriteOperation::ReplaceAll => Ok(content.to_string()),
        WriteOperation::InsertBeforeLine => {
            let target = start_line.ok_or("start_line is required for insert_before_line")?;
            if target == 0 {
                return Err("start_line must be 1 or greater".to_string());
            }
            let mut lines: Vec<String> = current_text.lines().map(|l| l.to_string()).collect();
            let insert_at = (target - 1).min(lines.len());
            let new_lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            for (i, new_line) in new_lines.into_iter().enumerate() {
                lines.insert(insert_at + i, new_line);
            }
            // Preserve trailing newline if original had one
            let mut result = lines.join("\n");
            if current_text.ends_with('\n') {
                result.push('\n');
            }
            Ok(result)
        }
        WriteOperation::ReplaceLines => {
            let start = start_line.ok_or("start_line is required for replace_lines")?;
            let end = end_line.ok_or("end_line is required for replace_lines")?;
            if start == 0 || end == 0 {
                return Err("start_line and end_line must be 1 or greater".to_string());
            }
            if start > end {
                return Err("start_line must be <= end_line".to_string());
            }
            let mut lines: Vec<String> = current_text.lines().map(|l| l.to_string()).collect();
            let replace_start = (start - 1).min(lines.len());
            let replace_end = end.min(lines.len());
            let new_lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            lines.splice(replace_start..replace_end, new_lines);
            let mut result = lines.join("\n");
            if current_text.ends_with('\n') {
                result.push('\n');
            }
            Ok(result)
        }
    }
}

fn tab_kind(tab: &TabType) -> &'static str {
    match tab {
        TabType::Query(_) => "query",
        TabType::Script(_) => "script",
        TabType::Settings(_) => "settings",
        TabType::Snippet(_) => "snippet",
        TabType::TableStructure(_) => "table_structure",
        TabType::ObjectDdl(_) => "object_ddl",
        TabType::SchemaGraph(_) => "schema_graph",
    }
}

/// The editor entity id doubles as the tab id: it is unique for the life of
/// the process and, unlike the index, survives tabs being closed around it.
fn tab_id(tab: &TabType) -> Option<u64> {
    tab.connection_tab()
        .map(|tab| tab.editor.entity_id().as_u64())
}

impl EditorPanel {
    pub(crate) fn tab_summaries(&self, cx: &App) -> Vec<TabSummary> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| self.summarize(index, tab, cx))
            .collect()
    }

    fn summarize(&self, index: usize, tab: &TabType, cx: &App) -> TabSummary {
        TabSummary {
            index,
            id: tab_id(tab),
            title: tab.title(cx),
            kind: tab_kind(tab),
            active: index == self.active_tab_ix,
            connection: tab.context(cx).as_ref().map(TabConnection::from),
        }
    }

    pub(crate) fn tab_summary_at(&self, index: usize, cx: &App) -> Result<TabSummary, String> {
        self.tabs
            .get(index)
            .map(|tab| self.summarize(index, tab, cx))
            .ok_or_else(|| self.out_of_range(index))
    }

    fn out_of_range(&self, index: usize) -> String {
        format!(
            "Tab index {index} is out of range, {} tabs are open. Use list_tabs.",
            self.tabs.len()
        )
    }

    /// Turn a selector into an index: by id first, then by index, else the
    /// active tab.
    pub(crate) fn resolve_tab(&self, selector: TabSelector) -> Result<usize, String> {
        if let Some(id) = selector.id {
            return self
                .tabs
                .iter()
                .position(|tab| tab_id(tab) == Some(id))
                .ok_or_else(|| format!("No tab with id {id}. Use list_tabs to find one."));
        }
        if let Some(index) = selector.index {
            return if index < self.tabs.len() {
                Ok(index)
            } else {
                Err(self.out_of_range(index))
            };
        }
        if self.tabs.is_empty() {
            return Err("No tabs are open.".to_string());
        }
        Ok(self.active_tab_ix)
    }

    fn editable_tab(&self, index: usize) -> Result<&ConnectionBackedTab, String> {
        let tab = self
            .tabs
            .get(index)
            .ok_or_else(|| self.out_of_range(index))?;
        tab.connection_tab().ok_or_else(|| {
            format!(
                "Tab {index} is a {} tab; only query and script tabs hold text.",
                tab_kind(tab)
            )
        })
    }

    pub(crate) fn read_tab_text(
        &self,
        index: usize,
        start_line: Option<usize>,
        end_line: Option<usize>,
        cx: &App,
    ) -> Result<TabContent, String> {
        let tab = self.editable_tab(index)?;
        let slice = slice_tab_text(tab.editor.read(cx).text(), start_line, end_line);
        Ok(TabContent {
            truncated: slice.truncated(),
            content: slice.content,
            total_lines: slice.total_lines,
            start_line: slice.start_line,
            end_line: slice.end_line,
        })
    }

    pub(crate) fn write_tab_text(
        &mut self,
        index: usize,
        edit: &TabEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WriteOutcome, String> {
        let editor = self.editable_tab(index)?.editor.clone();
        let total_lines = editor.update(cx, |editor, cx| {
            let current = editor.text().to_string();
            let new_text = apply_line_operation(
                &current,
                edit.operation,
                &edit.content,
                edit.start_line,
                edit.end_line,
            )?;
            editor.set_value(new_text, window, cx);
            Ok::<usize, String>(editor.text().lines_len())
        })?;
        cx.notify();
        Ok(WriteOutcome {
            message: edit.operation.summary(edit.start_line, edit.end_line),
            total_lines,
        })
    }

    /// Open a query tab on `context` and make it active. The default title is
    /// the database name, as the sidebar's "New Query" uses.
    pub(crate) fn create_query_tab_for_mcp(
        &mut self,
        context: ConnectionContext,
        content: Option<String>,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<TabSummary, String> {
        let title = title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| context.database_name.clone());
        self.create_and_add_tab_with_connection(
            window,
            TabCreationParams {
                title,
                content,
                db_id: None,
                last_run_at: None,
                context,
            },
            cx,
        );
        self.tab_summary_at(self.active_tab_ix, cx)
    }

    pub(crate) fn activate_tab_for_mcp(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<TabSummary, String> {
        if index >= self.tabs.len() {
            return Err(self.out_of_range(index));
        }
        self.set_active_tab(index, window, cx);
        self.tab_summary_at(index, cx)
    }

    /// Activate a query tab and press Run on it. The outcome arrives later as
    /// `EditorPanelEvent::QueryRunEnded`, since the run is asynchronous and may
    /// first open the PROD confirmation.
    pub(crate) fn run_query_tab_for_mcp(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        match self.tabs.get(index) {
            Some(TabType::Query(_)) => {}
            Some(other) => {
                return Err(format!(
                    "Tab {index} is a {} tab; run_tab only runs query tabs.",
                    tab_kind(other)
                ));
            }
            None => return Err(self.out_of_range(index)),
        }
        self.set_active_tab(index, window, cx);
        self.on_run_query(window, cx);
        Ok(())
    }
}
