use anyhow::Result;
use futures::StreamExt;
use sqlx::{Column, Row, TypeInfo, ValueRef};

use super::{PostgresConnection, QueryParam};
use blanco_core::{ColumnType, QueryResult};

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

        // Use raw_sql for queries without parameters, otherwise use query with bindings
        let mut results = if parameters.is_empty() {
            sqlx::raw_sql(sql_template).fetch_many(pool)
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
            query.fetch_many(pool)
        };

        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<ColumnType> = Vec::new();
        let mut raw_column_types: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut rows_affected: u64 = 0;

        // Track OIDs that need resolution: (row_idx, col_idx, oid_value)
        // Only store the minimal data needed instead of full raw rows
        let mut oids_to_resolve: Vec<(usize, usize, i32)> = Vec::new();

        while let Some(result) = results.next().await {
            match result? {
                Either::Left(execution_result) => {
                    rows_affected += execution_result.rows_affected();
                }
                Either::Right(row) => {
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
                    let row_idx = rows.len();
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

                    // Collect OIDs from Unknown (regclass) columns for later resolution
                    // We do this before pushing the row so we have the current row_idx
                    for (col_idx, col_type) in column_types.iter().enumerate() {
                        if *col_type == ColumnType::Unknown
                            && let Ok(raw_value) = row.try_get_raw(col_idx)
                            && !raw_value.is_null()
                        {
                            match raw_value.as_bytes() {
                                Ok(bytes) => {
                                    // PostgreSQL OIDs are 4-byte integers in network byte order (big-endian)
                                    if bytes.len() >= 4 {
                                        let oid = i32::from_be_bytes([
                                            bytes[0], bytes[1], bytes[2], bytes[3],
                                        ]);
                                        oids_to_resolve.push((row_idx, col_idx, oid));
                                    }
                                }
                                Err(_) => {
                                    // If we can't get bytes, we can't resolve this OID
                                }
                            }
                        }
                    }

                    rows.push(row_data);
                }
            }
        }

        // Resolve OIDs to table names if any were collected
        if !oids_to_resolve.is_empty() {
            let unique_oids: Vec<i32> = oids_to_resolve
                .iter()
                .map(|(_, _, oid)| *oid)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let oid_to_name = self.resolve_oids_to_names(pool, &unique_oids).await?;

            // Update data rows with resolved names
            for (row_idx, col_idx, oid) in oids_to_resolve {
                if let Some(name) = oid_to_name.get(&oid) {
                    rows[row_idx][col_idx] = Some(name.clone());
                }
            }
        }

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: Some(sql_template.to_string()),
            execution_time_ms: None,
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

        // Collect statement accumulators first, then finalize after the stream
        // is fully consumed. Finalization may issue a follow-up pg_class lookup
        // for regclass OIDs, and the pool is configured with a single
        // connection; doing that lookup while `fetch_many` still holds the
        // connection would deadlock.
        let mut accs: Vec<PgStatementAcc> = Vec::new();
        let mut current: Option<PgStatementAcc> = None;

        {
            let mut results = sqlx::raw_sql(sql).fetch_many(pool);
            while let Some(result) = results.next().await {
                match result? {
                    Either::Left(execution_result) => {
                        let mut acc = current.take().unwrap_or_default();
                        acc.rows_affected += execution_result.rows_affected();
                        accs.push(acc);
                    }
                    Either::Right(row) => {
                        let acc = current.get_or_insert_with(PgStatementAcc::default);
                        acc.absorb_row(&row, self);
                    }
                }
            }
            if let Some(acc) = current.take() {
                accs.push(acc);
            }
        }

        let mut out: Vec<QueryResult> = Vec::with_capacity(accs.len());
        for acc in accs {
            out.push(acc.finalize(pool, self).await?);
        }

        Ok(out)
    }
}

/// Per-statement accumulator used by `execute_script_inner` to keep rows,
/// columns, and pending OID lookups isolated to a single result-set.
#[derive(Default)]
struct PgStatementAcc {
    columns: Vec<String>,
    column_types: Vec<ColumnType>,
    raw_column_types: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    rows_affected: u64,
    oids_to_resolve: Vec<(usize, usize, i32)>,
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

        let row_idx = self.rows.len();
        let row_data: Vec<Option<String>> = (0..self.columns.len())
            .map(|i| {
                conn.convert_row_value_to_string(row, i, &self.column_types, &self.raw_column_types)
            })
            .collect();

        for (col_idx, col_type) in self.column_types.iter().enumerate() {
            if *col_type == ColumnType::Unknown
                && let Ok(raw_value) = row.try_get_raw(col_idx)
                && !raw_value.is_null()
                && let Ok(bytes) = raw_value.as_bytes()
                && bytes.len() >= 4
            {
                let oid = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                self.oids_to_resolve.push((row_idx, col_idx, oid));
            }
        }

        self.rows.push(row_data);
    }

    async fn finalize(
        mut self,
        pool: &sqlx::PgPool,
        conn: &PostgresConnection,
    ) -> Result<QueryResult> {
        if !self.oids_to_resolve.is_empty() {
            let unique_oids: Vec<i32> = self
                .oids_to_resolve
                .iter()
                .map(|(_, _, oid)| *oid)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let oid_to_name = conn.resolve_oids_to_names(pool, &unique_oids).await?;
            for (row_idx, col_idx, oid) in self.oids_to_resolve {
                if let Some(name) = oid_to_name.get(&oid) {
                    self.rows[row_idx][col_idx] = Some(name.clone());
                }
            }
        }

        Ok(QueryResult {
            columns: self.columns,
            column_types: self.column_types,
            rows: self.rows,
            rows_affected: self.rows_affected,
            query_text: None,
            execution_time_ms: None,
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        })
    }
}
