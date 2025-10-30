use gpui::{TestAppContext, Window, WindowOptions};
use blanco_core::QueryResult;
use crate::results_panel::{ResultsPanel, CellEditState, ResultsTableDelegate};
use crate::sql_parser::SqlTableExtractor;

#[gpui::test]
fn test_results_panel_creation(cx: &mut TestAppContext) {
    let window = cx.add_window(WindowOptions::default());
    let panel = cx.new(|cx| ResultsPanel::new(&mut window.clone(), cx));
    
    // Verify the panel was created successfully
    let panel_read = panel.read(cx);
    assert!(panel_read.current_result.is_none());
    assert!(panel_read.editing_input.is_none());
    assert!(panel_read.editing_cell.is_none());
}

#[gpui::test]
fn test_cell_edit_state(cx: &mut TestAppContext) {
    let mut edit_state = CellEditState::default();
    
    // Test initial state
    assert!(!edit_state.is_editing(0, 0));
    assert!(!edit_state.is_edited(0, 0));
    assert!(!edit_state.has_unsaved_changes());
    
    // Test starting editing
    edit_state.start_editing(0, 0, "original".to_string());
    assert!(edit_state.is_editing(0, 0));
    assert!(!edit_state.is_edited(0, 0));
    
    // Test updating value
    edit_state.update_editing_value(0, 0, "modified".to_string());
    assert!(edit_state.is_edited(0, 0));
    assert_eq!(edit_state.get_edited_value(0, 0), Some(&"modified".to_string()));
    assert_eq!(edit_state.get_original_value(0, 0), Some(&"original".to_string()));
    
    // Test committing edit
    let committed = edit_state.commit_edit(0, 0);
    assert_eq!(committed, Some("modified".to_string()));
    assert!(!edit_state.is_editing(0, 0));
    assert!(!edit_state.is_edited(0, 0));
}

#[gpui::test]
fn test_results_panel_set_query_result(cx: &mut TestAppContext) {
    let window = cx.add_window(WindowOptions::default());
    let mut panel = cx.new(|cx| ResultsPanel::new(&mut window.clone(), cx));
    
    // Create a test query result
    let test_result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string()],
        rows: vec![
            vec!["1".to_string(), "Alice".to_string()],
            vec!["2".to_string(), "Bob".to_string()],
        ],
        query_text: Some("SELECT * FROM users".to_string()),
        execution_time_ms: Some(50),
    };
    
    // Set the query result
    panel.update(cx, |panel, cx| {
        panel.set_query_result(test_result.clone(), None, cx);
    });
    
    // Verify the result was set
    let panel_read = panel.read(cx);
    assert!(panel_read.current_result.is_some());
    assert_eq!(panel_read.current_result.as_ref().unwrap().rows.len(), 2);
}

#[gpui::test]
fn test_results_panel_add_row(cx: &mut TestAppContext) {
    let window = cx.add_window(WindowOptions::default());
    let mut panel = cx.new(|cx| ResultsPanel::new(&mut window.clone(), cx));
    
    // Set initial data
    let test_result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string()],
        rows: vec![
            vec!["1".to_string(), "Alice".to_string()],
        ],
        query_text: Some("SELECT * FROM users".to_string()),
        execution_time_ms: Some(50),
    };
    
    panel.update(cx, |panel, cx| {
        panel.set_query_result(test_result, None, cx);
    });
    
    // Add a new row
    panel.update(cx, |panel, cx| {
        panel.add_new_row(cx);
    });
    
    // Verify the row was added
    let panel_read = panel.read(cx);
    let table_read = panel_read.table.read(cx);
    assert_eq!(table_read.delegate().rows.len(), 2);
    
    // Verify the new row is marked as pending
    assert!(table_read.delegate().edit_state.pending_new_rows.contains(&1));
}

#[gpui::test]
fn test_results_panel_duplicate_row(cx: &mut TestAppContext) {
    let window = cx.add_window(WindowOptions::default());
    let mut panel = cx.new(|cx| ResultsPanel::new(&mut window.clone(), cx));
    
    // Set initial data
    let test_result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string()],
        rows: vec![
            vec!["1".to_string(), "Alice".to_string()],
            vec!["2".to_string(), "Bob".to_string()],
        ],
        query_text: Some("SELECT * FROM users".to_string()),
        execution_time_ms: Some(50),
    };
    
    panel.update(cx, |panel, cx| {
        panel.set_query_result(test_result, None, cx);
    });
    
    // Duplicate the first row
    panel.update(cx, |panel, cx| {
        panel.duplicate_row(0, cx);
    });
    
    // Verify the row was duplicated
    let panel_read = panel.read(cx);
    let table_read = panel_read.table.read(cx);
    assert_eq!(table_read.delegate().rows.len(), 3);
    
    // Verify the duplicated row has the same data
    let duplicated_row = &table_read.delegate().rows[2];
    assert_eq!(duplicated_row[0], "1");
    assert_eq!(duplicated_row[1], "Alice");
    
    // Verify the new row is marked as pending
    assert!(table_read.delegate().edit_state.pending_new_rows.contains(&2));
}

#[gpui::test]
fn test_results_panel_cell_editing(cx: &mut TestAppContext) {
    let window = cx.add_window(WindowOptions::default());
    let mut panel = cx.new(|cx| ResultsPanel::new(&mut window.clone(), cx));
    
    // Set initial data
    let test_result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string()],
        rows: vec![
            vec!["1".to_string(), "Alice".to_string()],
        ],
        query_text: Some("SELECT * FROM users".to_string()),
        execution_time_ms: Some(50),
    };
    
    panel.update(cx, |panel, cx| {
        panel.set_query_result(test_result, None, cx);
    });
    
    // Start editing a cell
    panel.update(cx, |panel, cx| {
        panel.start_cell_edit(0, 1, &mut window.clone(), cx);
    });
    
    // Verify editing state
    let panel_read = panel.read(cx);
    assert!(panel_read.is_editing(cx));
    assert_eq!(panel_read.editing_cell, Some((0, 1)));
    assert!(panel_read.editing_input.is_some());
    
    // Commit the edit with a new value
    panel.update(cx, |panel, cx| {
        panel.commit_cell_edit(0, 1, "Alice Smith".to_string(), cx);
    });
    
    // Verify the edit was committed
    let panel_read = panel.read(cx);
    assert!(!panel_read.is_editing(cx));
    assert!(panel_read.has_unsaved_changes(cx));
    
    // Check the actual data was updated
    let table_read = panel_read.table.read(cx);
    assert_eq!(table_read.delegate().rows[0][1], "Alice Smith");
}

// SQL Parsing Tests
#[gpui::test]
fn test_sql_table_extraction_simple_select(cx: &mut TestAppContext) {
    let extractor = SqlTableExtractor::new();

    // Test basic SELECT
    assert_eq!(extractor.extract_primary_table("SELECT * FROM users").unwrap(), "users");
    assert_eq!(extractor.extract_primary_table("SELECT id, name FROM products").unwrap(), "products");

    // Test with whitespace
    assert_eq!(extractor.extract_primary_table("  SELECT   *   FROM    customers  ").unwrap(), "customers");
}

#[gpui::test]
fn test_sql_table_extraction_quoted_names(cx: &mut TestAppContext) {
    let extractor = SqlTableExtractor::new();

    // Test various quote styles
    assert_eq!(extractor.extract_primary_table("SELECT * FROM \"my-table\"").unwrap(), "my-table");
    assert_eq!(extractor.extract_primary_table("SELECT * FROM `table_name`").unwrap(), "table_name");
    assert_eq!(extractor.extract_primary_table("SELECT * FROM 'users'").unwrap(), "users");

    // Test database.schema.table format
    assert_eq!(extractor.extract_primary_table("SELECT * FROM mydb.users").unwrap(), "users");
    assert_eq!(extractor.extract_primary_table("SELECT * FROM public.customers").unwrap(), "customers");
}

#[gpui::test]
fn test_sql_table_extraction_complex_queries(cx: &mut TestAppContext) {
    let extractor = SqlTableExtractor::new();

    // Test JOIN queries - should extract the first table
    assert_eq!(
        extractor.extract_primary_table("SELECT u.*, p.* FROM users u JOIN profiles p ON u.id = p.user_id").unwrap(),
        "users"
    );

    // Test subqueries with alias
    assert_eq!(
        extractor.extract_primary_table("SELECT * FROM (SELECT * FROM users) AS t").unwrap(),
        "t"
    );

    // Test subqueries without alias
    assert_eq!(
        extractor.extract_primary_table("SELECT * FROM (SELECT * FROM products)").unwrap(),
        "products"
    );
}

#[gpui::test]
fn test_sql_table_extraction_all_statements(cx: &mut TestAppContext) {
    let extractor = SqlTableExtractor::new();

    // Test with JOIN - should return both tables
    let tables = extractor.extract_all_tables("SELECT u.*, p.* FROM users u JOIN profiles p ON u.id = p.user_id").unwrap();
    assert_eq!(tables, vec!["users", "profiles"]);

    // Test complex query with multiple tables
    let tables = extractor.extract_all_tables(
        "SELECT o.*, c.name FROM orders o JOIN customers c ON o.customer_id = c.id WHERE o.status = 'active'"
    ).unwrap();
    assert_eq!(tables, vec!["orders", "customers"]);
}

#[gpui::test]
fn test_sql_table_extraction_insert_update_delete(cx: &mut TestAppContext) {
    let extractor = SqlTableExtractor::new();

    // Test INSERT
    assert_eq!(
        extractor.extract_primary_table("INSERT INTO users (name, email) VALUES ('test', 'test@example.com')").unwrap(),
        "users"
    );

    // Test UPDATE
    assert_eq!(
        extractor.extract_primary_table("UPDATE users SET name = 'John' WHERE id = 1").unwrap(),
        "users"
    );

    // Test DELETE
    assert_eq!(
        extractor.extract_primary_table("DELETE FROM orders WHERE status = 'cancelled'").unwrap(),
        "orders"
    );
}

#[gpui::test]
fn test_results_table_delegate_table_name_extraction(cx: &mut TestAppContext) {
    let mut delegate = ResultsTableDelegate::default();

    // Test simple query
    let simple_result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string()],
        rows: vec![vec!["1".to_string(), "Alice".to_string()]],
        column_types: vec!["INTEGER".to_string(), "TEXT".to_string()],
        query_text: Some("SELECT * FROM users".to_string()),
        rows_affected: 0,
        execution_time_ms: None,
        is_error: false,
    };

    delegate.set_query_result(simple_result);

    // Verify table name was extracted
    assert_eq!(delegate.table_name.as_deref(), Some("users"));

    // Test complex query with JOIN
    let complex_result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
        rows: vec![vec!["1".to_string(), "Alice".to_string(), "alice@example.com".to_string()]],
        column_types: vec!["INTEGER".to_string(), "TEXT".to_string(), "TEXT".to_string()],
        query_text: Some("SELECT u.*, e.* FROM users u JOIN emails e ON u.id = e.user_id".to_string()),
        rows_affected: 0,
        execution_time_ms: None,
        is_error: false,
    };

    delegate.set_query_result(complex_result);

    // Should extract the first table from JOIN
    assert_eq!(delegate.table_name.as_deref(), Some("users"));

    // Test quoted table name
    let quoted_result = QueryResult {
        columns: vec!["id".to_string(), "status".to_string()],
        rows: vec![vec!["1".to_string(), "active".to_string()]],
        column_types: vec!["INTEGER".to_string(), "TEXT".to_string()],
        query_text: Some("SELECT * FROM \"user-orders\"".to_string()),
        rows_affected: 0,
        execution_time_ms: None,
        is_error: false,
    };

    delegate.set_query_result(quoted_result);

    // Should handle quoted names correctly
    assert_eq!(delegate.table_name.as_deref(), Some("user-orders"));
}

#[gpui::test]
fn test_results_table_delegate_primary_key_detection(cx: &mut TestAppContext) {
    let mut delegate = ResultsTableDelegate::default();

    // Test with explicit 'id' column
    let result_with_id = QueryResult {
        columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
        rows: vec![vec!["1".to_string(), "Alice".to_string(), "alice@example.com".to_string()]],
        column_types: vec!["INTEGER".to_string(), "TEXT".to_string(), "TEXT".to_string()],
        query_text: Some("SELECT * FROM users".to_string()),
        rows_affected: 0,
        execution_time_ms: None,
        is_error: false,
    };

    delegate.set_query_result(result_with_id);

    // Should detect 'id' as primary key
    assert_eq!(delegate.primary_key_column.as_deref(), Some("id"));

    // Test with 'uuid' column
    let result_with_uuid = QueryResult {
        columns: vec!["uuid".to_string(), "name".to_string(), "created_at".to_string()],
        rows: vec![vec!["123e4567-e89b-12d3-a456-426614174000".to_string(), "Bob".to_string(), "2023-01-01".to_string()]],
        column_types: vec!["TEXT".to_string(), "TEXT".to_string(), "TEXT".to_string()],
        query_text: Some("SELECT * FROM profiles".to_string()),
        rows_affected: 0,
        execution_time_ms: None,
        is_error: false,
    };

    delegate.set_query_result(result_with_uuid);

    // Should detect 'uuid' as primary key
    assert_eq!(delegate.primary_key_column.as_deref(), Some("uuid"));

    // Test with no obvious primary key
    let result_no_pk = QueryResult {
        columns: vec!["first_name".to_string(), "last_name".to_string(), "email".to_string()],
        rows: vec![vec!["John".to_string(), "Doe".to_string(), "john@example.com".to_string()]],
        column_types: vec!["TEXT".to_string(), "TEXT".to_string(), "TEXT".to_string()],
        query_text: Some("SELECT * FROM contacts".to_string()),
        rows_affected: 0,
        execution_time_ms: None,
        is_error: false,
    };

    delegate.set_query_result(result_no_pk);

    // Should return None when no primary key is detected
    assert_eq!(delegate.primary_key_column.as_deref(), None);
}