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
        let mut unnamed: Vec<(usize, u32)> = Vec::new();
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
                        unnamed = unnamed_columns(&row);
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
        drop(results);
        drop(connection);
        if !unnamed.is_empty() {
            let oids: Vec<u32> = unnamed.iter().map(|&(_, oid)| oid).collect();
            let names = type_names(pool, &oids).await?;
            for (column, oid) in unnamed {
                if let Some(name) = names.get(&oid) {
                    name_column(&mut column_types, &mut rows, column, name);
                }
            }
        }

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
        // Columns whose type the simple query protocol named only by OID,
        // as (result, column, OID): extension types such as hstore.
        let mut unnamed: Vec<(usize, usize, u32)> = Vec::new();
        let mut current: Option<PgStatementAcc> = None;
        let mut finish = |acc: PgStatementAcc, out: &mut Vec<QueryResult>| {
            unnamed.extend(
                acc.unnamed_types
                    .iter()
                    .map(|&(column, oid)| (out.len(), column, oid)),
            );
            out.push(acc.finish());
        };

        {
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
                        finish(acc, &mut out);
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
                finish(acc, &mut out);
            }
        }

        // The pool's one connection is free again now that the script is
        // read, so the names come from `pg_type` in one query.
        if !unnamed.is_empty() {
            let oids: Vec<u32> = unnamed.iter().map(|&(_, _, oid)| oid).collect();
            let names = type_names(pool, &oids).await?;
            for (result, column, oid) in unnamed {
                if let (Some(name), Some(result)) = (names.get(&oid), out.get_mut(result)) {
                    name_column(&mut result.column_types, &mut result.rows, column, name);
                }
            }
        }

        Ok(out)
    }
}

/// The columns of `row` whose type the simple query protocol gave only as
/// an OID, which sqlx names `?`: extension types such as hstore, which have
/// no fixed OID. As (column, OID).
fn unnamed_columns(row: &sqlx::postgres::PgRow) -> Vec<(usize, u32)> {
    row.columns()
        .iter()
        .enumerate()
        .filter(|(_, col)| col.type_info().name() == "?")
        .filter_map(|(ix, col)| Some((ix, col.type_info().oid()?.0)))
        .collect()
}

/// Give `column` the type `name`. The values are already the text the
/// server wrote and stay as they are, except PostGIS values: the server
/// writes those as hex EWKB, which becomes EWKT as on the binary protocol.
fn name_column(
    column_types: &mut [ColumnType],
    rows: &mut [Vec<Option<String>>],
    column: usize,
    name: &str,
) {
    let column_type = PostgresConnection::map_postgres_type(name);
    if column_type == ColumnType::Geometry {
        for value in rows.iter_mut().filter_map(|row| row.get_mut(column)) {
            if let Some(ewkt) = value
                .as_deref()
                .and_then(blanco_core::binary::decode_lenient)
                .and_then(|ewkb| blanco_core::geometry::ewkb_to_ewkt(&ewkb))
            {
                *value = Some(ewkt);
            }
        }
    }
    if let Some(slot) = column_types.get_mut(column) {
        *slot = column_type;
    }
}

/// The names of the types `oids`, from `pg_type`. Run once the statement's
/// rows are read: the pool holds a single connection.
async fn type_names(
    pool: &sqlx::PgPool,
    oids: &[u32],
) -> Result<std::collections::HashMap<u32, String>> {
    let oids: Vec<i64> = oids.iter().map(|&oid| i64::from(oid)).collect();
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT oid::int8, typname::text FROM pg_type WHERE oid::int8 = ANY($1)")
            .bind(&oids)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(oid, name)| Some((u32::try_from(oid).ok()?, name)))
        .collect())
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
    /// Columns the protocol named only by OID, with that OID.
    unnamed_types: Vec<(usize, u32)>,
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
            self.unnamed_types = unnamed_columns(row);
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
