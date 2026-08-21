//! A backend independent query plan tree, parsed from the result of a
//! dialect's EXPLAIN statement. PostgreSQL (`FORMAT JSON`), SQLite
//! (`EXPLAIN QUERY PLAN`) and MySQL (`FORMAT=TREE`) are supported; other
//! backends fall back to the raw tabular output.

use crate::{DatabaseType, QueryResult};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanNode {
    /// Operation name, e.g. `Seq Scan on users` or `SCAN users USING INDEX`.
    pub label: String,
    /// Secondary facts shown under the label (filter, index condition, ...).
    pub details: Vec<(String, String)>,
    pub estimated_rows: Option<f64>,
    pub actual_rows: Option<f64>,
    /// Inclusive wall time of this node in milliseconds, summed over loops.
    pub actual_time_ms: Option<f64>,
    pub total_cost: Option<f64>,
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    /// Time spent in this node only (inclusive time minus the children's),
    /// which is what makes a node "hot".
    pub fn self_time_ms(&self) -> Option<f64> {
        let inclusive = self.actual_time_ms?;
        let children: f64 = self
            .children
            .iter()
            .filter_map(|child| child.actual_time_ms)
            .sum();
        Some((inclusive - children).max(0.0))
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanTree {
    pub roots: Vec<PlanNode>,
    pub planning_time_ms: Option<f64>,
    pub execution_time_ms: Option<f64>,
}

impl PlanTree {
    /// The largest inclusive node time, used to scale the time bars.
    pub fn max_time_ms(&self) -> Option<f64> {
        fn walk(node: &PlanNode, max: &mut Option<f64>) {
            if let Some(time) = node.actual_time_ms {
                *max = Some(max.map_or(time, |current: f64| current.max(time)));
            }
            for child in &node.children {
                walk(child, max);
            }
        }
        let mut max = None;
        for root in &self.roots {
            walk(root, &mut max);
        }
        max
    }

    pub fn has_timings(&self) -> bool {
        self.max_time_ms().is_some()
    }
}

/// Parse the result of [`crate::DatabaseType`]'s EXPLAIN into a tree, or
/// `None` when the backend's output is not structured (or did not parse).
pub fn parse_plan(db_type: DatabaseType, result: &QueryResult) -> Option<PlanTree> {
    match db_type {
        DatabaseType::PostgreSQL => parse_postgres(result),
        DatabaseType::SQLite => parse_sqlite(result),
        DatabaseType::MySQL => parse_mysql_tree(result),
        _ => None,
    }
}

fn single_cell(result: &QueryResult) -> Option<String> {
    // Some drivers return one row per line of output; join them back.
    let cells: Vec<&str> = result
        .rows
        .iter()
        .filter_map(|row| row.first().and_then(|cell| cell.as_deref()))
        .collect();
    if cells.is_empty() {
        None
    } else {
        Some(cells.join("\n"))
    }
}

fn parse_postgres(result: &QueryResult) -> Option<PlanTree> {
    let text = single_cell(result)?;
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let entries = value.as_array()?;
    let mut tree = PlanTree::default();
    for entry in entries {
        let plan = entry.get("Plan")?;
        tree.roots.push(postgres_node(plan));
        tree.planning_time_ms = entry.get("Planning Time").and_then(|v| v.as_f64());
        tree.execution_time_ms = entry.get("Execution Time").and_then(|v| v.as_f64());
    }
    Some(tree)
}

fn postgres_node(plan: &serde_json::Value) -> PlanNode {
    let get_str = |key: &str| plan.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let get_f64 = |key: &str| plan.get(key).and_then(|v| v.as_f64());

    let node_type = get_str("Node Type").unwrap_or_else(|| "Node".to_string());
    let mut label = node_type;
    if let Some(join_type) = get_str("Join Type") {
        label = format!("{join_type} {label}");
    }
    if let Some(relation) = get_str("Relation Name") {
        label.push_str(" on ");
        label.push_str(&relation);
        if let Some(alias) = get_str("Alias").filter(|alias| *alias != relation) {
            label.push(' ');
            label.push_str(&alias);
        }
    }
    if let Some(index) = get_str("Index Name") {
        label.push_str(" using ");
        label.push_str(&index);
    }

    let mut details = Vec::new();
    for (key, title) in [
        ("Filter", "Filter"),
        ("Index Cond", "Index Cond"),
        ("Hash Cond", "Hash Cond"),
        ("Merge Cond", "Merge Cond"),
        ("Join Filter", "Join Filter"),
        ("Recheck Cond", "Recheck Cond"),
        ("Sort Key", "Sort Key"),
        ("Group Key", "Group Key"),
        ("Rows Removed by Filter", "Rows Removed by Filter"),
        ("Heap Fetches", "Heap Fetches"),
        ("Sort Method", "Sort Method"),
    ] {
        if let Some(value) = plan.get(key) {
            let text = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                other => other.to_string(),
            };
            details.push((title.to_string(), text));
        }
    }

    let loops = get_f64("Actual Loops").unwrap_or(1.0);
    let actual_time_ms = get_f64("Actual Total Time").map(|time| time * loops);
    let actual_rows = get_f64("Actual Rows").map(|rows| rows * loops);

    PlanNode {
        label,
        details,
        estimated_rows: get_f64("Plan Rows"),
        actual_rows,
        actual_time_ms,
        total_cost: get_f64("Total Cost"),
        children: plan
            .get("Plans")
            .and_then(|plans| plans.as_array())
            .map(|plans| plans.iter().map(postgres_node).collect())
            .unwrap_or_default(),
    }
}

/// `EXPLAIN QUERY PLAN` rows are `(id, parent, notused, detail)`.
fn parse_sqlite(result: &QueryResult) -> Option<PlanTree> {
    let id_col = result.columns.iter().position(|c| c == "id")?;
    let parent_col = result.columns.iter().position(|c| c == "parent")?;
    let detail_col = result.columns.iter().position(|c| c == "detail")?;

    let mut entries: Vec<(i64, i64, String)> = Vec::new();
    for row in &result.rows {
        let id = row.get(id_col)?.as_deref()?.parse().ok()?;
        let parent = row.get(parent_col)?.as_deref()?.parse().ok()?;
        let detail = row.get(detail_col)?.as_deref()?.to_string();
        entries.push((id, parent, detail));
    }

    fn build(parent: i64, entries: &[(i64, i64, String)]) -> Vec<PlanNode> {
        entries
            .iter()
            .filter(|(_, p, _)| *p == parent)
            .map(|(id, _, detail)| PlanNode {
                label: detail.clone(),
                children: build(*id, entries),
                ..PlanNode::default()
            })
            .collect()
    }

    let roots = build(0, &entries);
    if roots.is_empty() {
        return None;
    }
    Some(PlanTree {
        roots,
        ..PlanTree::default()
    })
}

/// `EXPLAIN FORMAT=TREE` is indented text where each node starts with `-> `.
/// Nesting is derived from the indentation of the arrow.
fn parse_mysql_tree(result: &QueryResult) -> Option<PlanTree> {
    let text = single_cell(result)?;
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    let mut roots: Vec<PlanNode> = Vec::new();

    fn pop_into_parent(stack: &mut Vec<(usize, PlanNode)>, roots: &mut Vec<PlanNode>) {
        if let Some((_, node)) = stack.pop() {
            match stack.last_mut() {
                Some((_, parent)) => parent.children.push(node),
                None => roots.push(node),
            }
        }
    }

    for line in text.lines() {
        let Some(arrow) = line.find("-> ") else {
            continue;
        };
        let indent = arrow;
        let body = line[arrow + 3..].trim();
        let node = mysql_node(body);
        while stack.last().is_some_and(|(depth, _)| *depth >= indent) {
            pop_into_parent(&mut stack, &mut roots);
        }
        stack.push((indent, node));
    }
    while !stack.is_empty() {
        pop_into_parent(&mut stack, &mut roots);
    }
    if roots.is_empty() {
        return None;
    }
    Some(PlanTree {
        roots,
        ..PlanTree::default()
    })
}

/// `Table scan on t  (cost=1.2 rows=3) (actual time=0.1..0.5 rows=3 loops=1)`
fn mysql_node(body: &str) -> PlanNode {
    let label_end = body.find("  (").unwrap_or(body.len());
    let label = body[..label_end].trim().to_string();
    let rest = &body[label_end..];

    let field = |name: &str| -> Option<f64> {
        let start = rest.find(&format!("{name}="))? + name.len() + 1;
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c == ' ' || c == ')')
            .unwrap_or(tail.len());
        // Ranges like `0.1..0.5` report the end value.
        let token = &tail[..end];
        let value = token.rsplit("..").next()?;
        value.parse().ok()
    };
    let loops = field("loops").unwrap_or(1.0);
    let (estimated_rows, actual_rows) = match rest.find("actual time") {
        Some(actual_start) => {
            let estimate = rest[..actual_start].find("rows=").and_then(|i| {
                rest[..actual_start][i + 5..]
                    .split(|c: char| c == ' ' || c == ')')
                    .next()?
                    .parse()
                    .ok()
            });
            let actual = rest[actual_start..].find("rows=").and_then(|i| {
                rest[actual_start + i + 5..]
                    .split(|c: char| c == ' ' || c == ')')
                    .next()?
                    .parse::<f64>()
                    .ok()
            });
            (estimate, actual.map(|rows| rows * loops))
        }
        None => (field("rows"), None),
    };
    let actual_time_ms = rest.find("actual time=").and_then(|i| {
        let tail = &rest[i + "actual time=".len()..];
        let end = tail.find(' ').unwrap_or(tail.len());
        tail[..end].rsplit("..").next()?.parse::<f64>().ok()
    });

    PlanNode {
        label,
        estimated_rows,
        actual_rows,
        actual_time_ms: actual_time_ms.map(|time| time * loops),
        total_cost: field("cost"),
        ..PlanNode::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(columns: &[&str], rows: Vec<Vec<&str>>) -> QueryResult {
        QueryResult {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(|c| Some(c.to_string())).collect())
                .collect(),
            ..QueryResult::default()
        }
    }

    #[test]
    fn postgres_json_plan() {
        let json = r#"[{"Plan":{"Node Type":"Hash Join","Join Type":"Inner","Hash Cond":"(o.user_id = u.id)","Plan Rows":10,"Actual Rows":12,"Actual Total Time":1.5,"Actual Loops":1,"Total Cost":20.5,"Plans":[{"Node Type":"Seq Scan","Relation Name":"orders","Alias":"o","Plan Rows":100,"Actual Rows":100,"Actual Total Time":0.4,"Actual Loops":1,"Total Cost":5.0,"Filter":"(amount > 10)"},{"Node Type":"Index Scan","Relation Name":"users","Alias":"u","Index Name":"users_pkey","Plan Rows":1,"Actual Rows":1,"Actual Total Time":0.01,"Actual Loops":100,"Total Cost":0.3}]},"Planning Time":0.2,"Execution Time":2.0}]"#;
        let tree = parse_plan(
            DatabaseType::PostgreSQL,
            &result(&["QUERY PLAN"], vec![vec![json]]),
        )
        .expect("plan");
        assert_eq!(tree.execution_time_ms, Some(2.0));
        let root = &tree.roots[0];
        assert_eq!(root.label, "Inner Hash Join");
        assert_eq!(
            root.details[0],
            ("Hash Cond".into(), "(o.user_id = u.id)".into())
        );
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].label, "Seq Scan on orders o");
        assert_eq!(
            root.children[1].label,
            "Index Scan on users u using users_pkey"
        );
        // Loops are folded into the totals.
        assert_eq!(root.children[1].actual_time_ms, Some(1.0));
        assert_eq!(root.children[1].actual_rows, Some(100.0));
        assert!((root.self_time_ms().unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(tree.max_time_ms(), Some(1.5));
    }

    #[test]
    fn sqlite_query_plan_rows_become_a_tree() {
        let rows = result(
            &["id", "parent", "notused", "detail"],
            vec![
                vec!["3", "0", "0", "SCAN orders"],
                vec![
                    "7",
                    "3",
                    "0",
                    "SEARCH users USING INTEGER PRIMARY KEY (rowid=?)",
                ],
            ],
        );
        let tree = parse_plan(DatabaseType::SQLite, &rows).expect("plan");
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].label, "SCAN orders");
        assert_eq!(
            tree.roots[0].children[0].label,
            "SEARCH users USING INTEGER PRIMARY KEY (rowid=?)"
        );
        assert!(!tree.has_timings());
    }

    #[test]
    fn mysql_tree_format() {
        let text = "-> Nested loop inner join  (cost=2.5 rows=4) (actual time=0.05..0.20 rows=4 loops=1)\n    -> Table scan on o  (cost=1.0 rows=4) (actual time=0.02..0.05 rows=4 loops=1)\n    -> Single-row index lookup on u using PRIMARY (id=o.user_id)  (cost=0.3 rows=1) (actual time=0.01..0.01 rows=1 loops=4)\n";
        let tree =
            parse_plan(DatabaseType::MySQL, &result(&["EXPLAIN"], vec![vec![text]])).expect("plan");
        assert_eq!(tree.roots.len(), 1);
        let root = &tree.roots[0];
        assert_eq!(root.label, "Nested loop inner join");
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.total_cost, Some(2.5));
        assert_eq!(root.estimated_rows, Some(4.0));
        assert_eq!(root.actual_rows, Some(4.0));
        assert_eq!(root.actual_time_ms, Some(0.2));
        assert_eq!(root.children[1].actual_time_ms, Some(0.04));
    }

    #[test]
    fn unsupported_backends_return_none() {
        assert!(parse_plan(DatabaseType::ClickHouse, &result(&["x"], vec![vec!["y"]])).is_none());
        assert!(
            parse_plan(
                DatabaseType::PostgreSQL,
                &result(&["x"], vec![vec!["not json"]])
            )
            .is_none()
        );
    }
}
