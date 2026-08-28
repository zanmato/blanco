use database::DatabaseType;
use gpui::{TestAppContext, VisualTestContext};

use crate::test_harness::{TestHarness, set_editor_text};

/// A Redis-backed tab must use the `redis` editor language and render a
/// multi-line command buffer without panicking. Loading the `redis` highlights
/// query happens during render, and a query that doesn't match the grammar
/// panics at load, so this guards against `highlights.scm` drift end to end.
#[gpui::test]
async fn redis_tab_renders_with_highlighting(cx: &mut TestAppContext) {
    let harness = TestHarness::new_with_db_type(cx, DatabaseType::Redis);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SET key \"hello world\"\n# 2026-06-30 14:04:35, 510 rows in 539us\nCONFIG GET maxmemory\nLRANGE mylist 0 -1\nTOTALLYNOTACOMMAND arg",
        &mut cx,
    );

    cx.run_until_parked();

    harness.editor_panel.read_with(&cx, |panel, cx| {
        let tab = panel.active_query_tab().expect("redis tab should exist");
        assert_eq!(tab.context.db_type, DatabaseType::Redis);
        assert_eq!(tab.context.db_type.editor_language(), "redis");
        let text = tab.editor.read(cx).text().to_string();
        assert!(text.contains("CONFIG GET maxmemory"));
        // Redis tabs must not get the SQL completion provider.
        assert!(
            tab.completion_provider.is_none(),
            "Redis tab should not have a SQL completion provider"
        );
    });
}
