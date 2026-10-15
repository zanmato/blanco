use anyhow::Result;
use futures::StreamExt;
use sqlx::{Column, Row, TypeInfo};

use super::{PostgresConnection, QueryParam};
use blanco_core::{ColumnType, QueryResult, QueryTiming, StatementTimer};

impl PostgresConnection {
    /// Helper method to execute a query with parameters
    pub(crate) async fn execute_query_with_params(
        &self,
        pool: &sqlx::PgPool,
        sql_template: &str,
        parameters: &[QueryParam],
    ) -> Result<QueryResult> {
        use sqlx::Either;

        tracing::debug!(
            "Executing PostgreSQL query with {} parameters",
            parameters.len()
        );

        // Taken before the timer starts, so opening a connection does not
        // count as execution.
        let mut connection = pool.acquire().await?;
        let mut timer = StatementTimer::start();

        // Use raw_sql for queries without parameters, otherwise use query with bindings
        let mut results = if parameters.is_empty() {
            sqlx::raw_sql(sql_template).fetch_many(&mut *connection)
        } else {
            let mut query = sqlx::query(sql_template);
            for param in parameters {
                query = match param {
                    QueryParam::String(s) => query.bind(s.as_str()),
                    QueryParam::StringArray(arr) => query.bind(arr.as_slice()),
                    QueryParam::I64(n) => query.bind(*n),
                    QueryParam::I32(n) => query.bind(*n),
                    QueryParam::F64(n) => query.bind(*n),
                    QueryParam::Bool(b) => query.bind(*b),
                };
            }
            #[allow(deprecated)]
            query.fetch_many(&mut *connection)
        };

        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<ColumnType> = Vec::new();
        let mut raw_column_types: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut rows_affected: u64 = 0;

        while let Some(result) = results.next().await {
            match result? {
                Either::Left(execution_result) => {
                    rows_affected += execution_result.rows_affected();
                }
                Either::Right(row) => {
                    timer.row();
                    // Extract column info from the first row
                    if columns.is_empty() {
                        columns = row
                            .columns()
                            .iter()
                            .map(|col| col.name().to_string())
                            .collect();

                        let (types, raw_types): (Vec<ColumnType>, Vec<String>) = row
                            .columns()
                            .iter()
                            .map(|col| {
                                let raw_type = col.type_info().name().to_string();
                                (Self::map_postgres_type(&raw_type), raw_type)
                            })
                            .unzip();
                        column_types = types;
                        raw_column_types = raw_types;
                    }

                    // Convert row to strings immediately to avoid memory doubling
                    let row_data: Vec<Option<String>> = (0..columns.len())
                        .map(|i| {
                            self.convert_row_value_to_string(
                                &row,
                                i,
                                &column_types,
                                &raw_column_types,
                            )
                        })
                        .collect();

                    rows.push(row_data);
                }
            }
        }
        let timing = timer.finish();

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: Some(sql_template.to_string()),
            timing: Some(timing),
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        })
    }

    /// Execute a script that may contain multiple statements, returning one
    /// `QueryResult` per statement. Each `Either::Left` from sqlx marks a
    /// statement boundary.
    pub(crate) async fn execute_script_inner(
        &self,
        pool: &sqlx::PgPool,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        use sqlx::Either;

        let mut out: Vec<QueryResult> = Vec::new();
        let mut current: Option<PgStatementAcc> = None;

        // Taken before the timer starts, so opening a connection does not
        // count as execution.
        let mut connection = pool.acquire().await?;
        let mut timer = StatementTimer::start();
        let mut results = sqlx::raw_sql(sql).fetch_many(&mut *connection);
        while let Some(result) = results.next().await {
            match result? {
                Either::Left(execution_result) => {
                    let mut acc = current.take().unwrap_or_default();
                    acc.rows_affected += execution_result.rows_affected();
                    acc.timing = Some(timer.finish_and_restart());
                    out.push(acc.finish());
                }
                Either::Right(row) => {
                    timer.row();
                    let acc = current.get_or_insert_with(PgStatementAcc::default);
                    acc.absorb_row(&row, self);
                }
            }
        }
        if let Some(mut acc) = current.take() {
            acc.timing = Some(timer.finish());
            out.push(acc.finish());
        }

        Ok(out)
    }
}

/// Per-statement accumulator used by `execute_script_inner` to keep rows and
/// columns isolated to a single result-set.
#[derive(Default)]
struct PgStatementAcc {
    columns: Vec<String>,
    column_types: Vec<ColumnType>,
    raw_column_types: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    rows_affected: u64,
    timing: Option<QueryTiming>,
}

impl PgStatementAcc {
    fn absorb_row(&mut self, row: &sqlx::postgres::PgRow, conn: &PostgresConnection) {
        if self.columns.is_empty() {
            self.columns = row
                .columns()
                .iter()
                .map(|col| col.name().to_string())
                .collect();
            let (types, raw_types): (Vec<ColumnType>, Vec<String>) = row
                .columns()
                .iter()
                .map(|col| {
                    let raw_type = col.type_info().name().to_string();
                    (PostgresConnection::map_postgres_type(&raw_type), raw_type)
                })
                .unzip();
            self.column_types = types;
            self.raw_column_types = raw_types;
        }

        let row_data: Vec<Option<String>> = (0..self.columns.len())
            .map(|i| {
                conn.convert_row_value_to_string(row, i, &self.column_types, &self.raw_column_types)
            })
            .collect();
        self.rows.push(row_data);
    }

    fn finish(self) -> QueryResult {
        QueryResult {
            columns: self.columns,
            column_types: self.column_types,
            rows: self.rows,
            rows_affected: self.rows_affected,
            query_text: None,
            timing: self.timing,
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        }
    }
}
