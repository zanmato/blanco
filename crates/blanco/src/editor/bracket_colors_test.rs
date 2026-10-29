use std::ops::Range;

use gpui::{TestAppContext, VisualTestContext};

use crate::app_settings::AppSettings;
use crate::test_harness::{TestHarness, set_editor_text};

fn colored(harness: &TestHarness, cx: &VisualTestContext) -> Vec<Range<usize>> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        panel
            .active_query_tab()
            .and_then(|tab| tab.bracket_colors.as_ref())
            .map(|colors| colors.decorated_ranges(cx))
            .unwrap_or_default()
    })
}

/// Lets the outline and the bracket scan finish on their background threads
/// and the editor lay out, which the decorations wait for.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..5 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
    }
    cx.run_until_parked();
}

/// Put `text` in the editor with the cursor where `|` stands, and return the
/// colored brackets as the bracket and its depth, `None` for one without a
/// partner.
fn colors_at_cursor(
    harness: &TestHarness,
    text_with_cursor: &str,
    cx: &mut VisualTestContext,
) -> Vec<(char, Option<u32>)> {
    let cursor = text_with_cursor
        .find('|')
        .expect("the text marks the cursor with |");
    let text = text_with_cursor.replacen('|', "", 1);
    set_editor_text(harness, &text, cx);
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        let tab = panel.active_query_tab().expect("query tab");
        tab.editor.update(cx, |state, cx| {
            state.set_selected_range(cursor..cursor, window, cx);
        });
    });
    settle(cx);
    harness.editor_panel.read_with(cx, |panel, cx| {
        panel
            .active_query_tab()
            .and_then(|tab| tab.bracket_colors.as_ref())
            .map(|colors| colors.decorated_depths(cx))
            .unwrap_or_default()
            .into_iter()
            .map(|(offset, depth)| (char::from(text.as_bytes()[offset]), depth))
            .collect()
    })
}

const PAIR: [(char, Option<u32>); 2] = [('(', Some(0)), (')', Some(0))];

#[gpui::test]
async fn only_the_statement_under_the_cursor_is_colored(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let script = "SELECT REPLACE('hello', 'world', yes);\n\nSELECT REPLACE()";
    let second = script.to_string() + "|";
    assert_eq!(colors_at_cursor(&harness, &second, &mut cx), PAIR);
    let first = script.replacen("yes", "yes|", 1);
    assert_eq!(colors_at_cursor(&harness, &first, &mut cx), PAIR);
    let first_pair: Vec<_> = [14, 36].map(|offset| offset..offset + 1).into();
    assert_eq!(colored(&harness, &cx), first_pair);

    // Moving the cursor alone moves the colors to the other statement.
    harness.editor_panel.update_in(&mut cx, |panel, window, cx| {
        let tab = panel.active_query_tab().expect("query tab");
        tab.editor.update(cx, |state, cx| {
            state.set_selected_range(script.len()..script.len(), window, cx)
        });
    });
    settle(&mut cx);
    let second_pair: Vec<_> = [54, 55].map(|offset| offset..offset + 1).into();
    assert_eq!(colored(&harness, &cx), second_pair);
}

#[gpui::test]
async fn both_brackets_of_a_pair_are_colored(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    assert_eq!(
        colors_at_cursor(&harness, "SELECT REPLACE(|)", &mut cx),
        PAIR
    );
    assert_eq!(
        colors_at_cursor(&harness, "SELECT REPLACE(|);", &mut cx),
        PAIR
    );
    assert_eq!(
        colors_at_cursor(
            &harness,
            "SELECT REPLACE(REPLACE(description, '(', ''), ')', '')|",
            &mut cx
        ),
        vec![
            ('(', Some(0)),
            ('(', Some(1)),
            (')', Some(1)),
            (')', Some(0))
        ]
    );
}

#[gpui::test]
async fn an_unclosed_string_leaves_its_bracket_without_a_partner(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    assert_eq!(
        colors_at_cursor(&harness, "SELECT REPLACE('hello', 'world', ';|", &mut cx),
        vec![('(', None)]
    );
    assert_eq!(
        colors_at_cursor(&harness, "SELECT (1 + 2|", &mut cx),
        vec![('(', None)]
    );
    assert_eq!(
        colors_at_cursor(&harness, "SELECT 1 + 2)|", &mut cx),
        vec![(')', None)]
    );
}

#[gpui::test]
async fn statements_of_every_kind_are_colored(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let depths = |depths: &[(char, u32)]| -> Vec<(char, Option<u32>)> {
        depths
            .iter()
            .map(|(bracket, depth)| (*bracket, Some(*depth)))
            .collect()
    };

    assert_eq!(
        colors_at_cursor(
            &harness,
            "WITH totals AS (SELECT COUNT(*) AS n FROM t)\nSELECT n| FROM totals",
            &mut cx
        ),
        depths(&[('(', 0), ('(', 1), (')', 1), (')', 0)])
    );
    assert_eq!(
        colors_at_cursor(
            &harness,
            "INSERT INTO t (a, b) VALUES (1, lower('X')), (2|, 'y');",
            &mut cx
        ),
        depths(&[
            ('(', 0),
            (')', 0),
            ('(', 0),
            ('(', 1),
            (')', 1),
            (')', 0),
            ('(', 0),
            (')', 0),
        ])
    );
    assert_eq!(
        colors_at_cursor(
            &harness,
            "CREATE TABLE t (\n  id INTEGER PRIMARY KEY,\n  price NUMERIC(10, 2) CHECK (price > 0)|\n);",
            &mut cx
        ),
        depths(&[('(', 0), ('(', 1), (')', 1), ('(', 1), (')', 1), (')', 0)])
    );
    assert_eq!(
        colors_at_cursor(
            &harness,
            "SELECT * FROM t WHERE id IN (SELECT id FROM u WHERE (a OR b)|) AND c = 1",
            &mut cx
        ),
        depths(&[('(', 0), ('(', 1), (')', 1), (')', 0)])
    );
    assert_eq!(
        colors_at_cursor(
            &harness,
            "UPDATE t SET total = (CASE WHEN x > 0 THEN abs(x) ELSE 0 END)|\nWHERE id = 1;",
            &mut cx
        ),
        depths(&[('(', 0), ('(', 1), (')', 1), (')', 0)])
    );
}

/// A large INSERT hands the editor decorations only for the rows around the
/// visible ones, since the editor moves every decoration it holds on each
/// keystroke.
#[gpui::test]
async fn only_rows_near_the_screen_are_colored(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let rows: Vec<String> = (0..20_000)
        .map(|ix| format!("({ix}, 'name {ix}', ({ix} + 1))"))
        .collect();
    let sql = format!(
        "INSERT INTO t (id, name, total) VALUES\n{};",
        rows.join(",\n")
    );
    set_editor_text(&harness, &sql, &mut cx);
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            let tab = panel.active_query_tab().expect("query tab");
            tab.editor
                .update(cx, |state, cx| state.set_selected_range(0..0, window, cx));
        });
    settle(&mut cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    settle(&mut cx);

    let colored = colored(&harness, &cx);
    assert!(!colored.is_empty(), "the visible rows should be colored");
    assert!(
        colored.len() < 2_000,
        "{} brackets handed to the editor",
        colored.len()
    );
    let all: Vec<_> = sql
        .match_indices(['(', ')'])
        .map(|(offset, _)| offset..offset + 1)
        .collect();
    assert_eq!(
        colored,
        all[..colored.len()],
        "the first rows are on screen"
    );
}

#[gpui::test]
async fn the_setting_turns_colors_off(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    assert_eq!(colors_at_cursor(&harness, "SELECT (1)|", &mut cx), PAIR);

    cx.update(|_, cx| {
        AppSettings::global_mut(cx)
            .settings
            .editor
            .colorize_brackets = false;
    });
    settle(&mut cx);
    assert!(colored(&harness, &cx).is_empty());

    cx.update(|_, cx| {
        AppSettings::global_mut(cx)
            .settings
            .editor
            .colorize_brackets = true;
    });
    settle(&mut cx);
    assert_eq!(colored(&harness, &cx).len(), 2);
}
