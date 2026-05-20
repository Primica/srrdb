use std::sync::Arc;
use std::sync::Mutex;

use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, DataType, Expr, FromTable, Ident, ObjectName,
    ObjectNamePart, ObjectType, OrderByExpr, Query, SelectItem, SetExpr, Statement,
    TableFactor, Value as SqlValue,
};
use sqlparser::ast::Use as SqlUse;
use tracing::{error, info};

use crate::engine::catalog::Catalog;
use crate::engine::persistence::Persistence;
use crate::engine::storage::{Row, Storage};
use crate::engine::types::{Column, ColumnType, Value};
use crate::engine::wal::{Wal, WalEntry};

#[derive(Debug)]
pub enum ExecuteResult {
    Rows {
        columns: Vec<Column>,
        rows: Vec<Row>,
    },
    Affected {
        rows: u64,
        last_insert_id: u64,
    },
    DatabaseChanged(String),
    Ok,
}

pub struct Executor {
    pub catalog: Arc<Mutex<Catalog>>,
    pub storage: Arc<Mutex<Storage>>,
    pub persistence: Option<Persistence>,
    pub wal: Arc<Mutex<Wal>>,
}

impl Executor {
    pub fn new(catalog: Arc<Mutex<Catalog>>, storage: Arc<Mutex<Storage>>) -> Self {
        let wal = Arc::new(Mutex::new(Wal::new(&std::path::PathBuf::from("data"))));
        Executor { catalog, storage, persistence: None, wal }
    }

    pub fn with_persistence(
        catalog: Arc<Mutex<Catalog>>,
        storage: Arc<Mutex<Storage>>,
        persistence: Persistence,
    ) -> Self {
        let wal = Arc::new(Mutex::new(Wal::new(persistence.data_dir())));
        Executor { catalog, storage, persistence: Some(persistence), wal }
    }

    pub fn with_wal(
        catalog: Arc<Mutex<Catalog>>,
        storage: Arc<Mutex<Storage>>,
        persistence: Persistence,
        wal: Arc<Mutex<Wal>>,
    ) -> Self {
        Executor { catalog, storage, persistence: Some(persistence), wal }
    }

    pub fn execute(&self, db: &str, statement: &Statement) -> Result<ExecuteResult, String> {
        match statement {
            Statement::CreateTable(ct) => {
                let table_name = name_to_string(&ct.name);
                if ct.if_not_exists && self.table_exists(db, &table_name) {
                    return Ok(ExecuteResult::Ok);
                }
                self.execute_create_table(db, ct)
            }
            Statement::Insert(ins) => {
                let table = table_object_name(&ins.table);
                self.execute_insert(db, &table, &ins.columns, &ins.source)
            }
            Statement::Query(query) => self.execute_query(db, query),
            Statement::Delete(del) => {
                let table = del.tables.first()
                    .map(name_to_string)
                    .or_else(|| {
                        match &del.from {
                            FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => {
                                tables.first().and_then(|t| match &t.relation {
                                    TableFactor::Table { name, .. } => Some(name_to_string(name)),
                                    _ => None,
                                })
                            }
                        }
                    })
                    .unwrap_or_default();
                self.execute_delete(db, &table, &del.selection)
            }
            Statement::Update {
                table,
                assignments,
                selection,
                ..
            } => {
                let table_name = table_factor_name(&table.relation);
                self.execute_update(db, &table_name, assignments, selection)
            }
            Statement::Drop {
                object_type,
                names,
                if_exists,
                ..
            } if *object_type == ObjectType::Table => {
                let table = names.first().map(name_to_string).unwrap_or_default();
                if *if_exists && !self.table_exists(db, &table) {
                    return Ok(ExecuteResult::Ok);
                }
                self.execute_drop_table(db, &table)
            }
            Statement::Drop {
                object_type,
                names,
                if_exists,
                ..
            } if *object_type == ObjectType::Database => {
                let db_name = names.first().map(name_to_string).unwrap_or_default();
                if *if_exists && !self.database_exists(&db_name) {
                    return Ok(ExecuteResult::Ok);
                }
                self.execute_drop_database(&db_name)
            }
            Statement::CreateDatabase { db_name, if_not_exists, .. } => {
                let name = name_to_string(db_name);
                if *if_not_exists && self.database_exists(&name) {
                    return Ok(ExecuteResult::Ok);
                }
                self.execute_create_database(db_name)
            }
            Statement::ShowDatabases { .. } => self.execute_show_databases(),
            Statement::ShowTables { .. } => self.execute_show_tables(db),
            Statement::Use(use_stmt) => self.execute_use(use_stmt),
            _ => Err(format!("Unsupported statement")),
        }
    }

    fn log_wal(&self, entry: &WalEntry) {
        if let Ok(mut wal) = self.wal.lock() {
            if let Err(e) = wal.append(entry) {
                error!("WAL append failed: {e}");
            }
        }
    }

    fn save(&self) {
        if let Some(ref persistence) = self.persistence {
            let catalog = match self.catalog.lock() {
                Ok(c) => c,
                Err(e) => {
                    error!("Catalog lock poisoned: {e}");
                    return;
                }
            };
            if let Err(e) = persistence.save_catalog(&catalog) {
                error!("Failed to save catalog: {e}");
            }
        }
    }

    fn save_table(&self, table_name: &str) {
        if let Some(ref persistence) = self.persistence {
            let storage = match self.storage.lock() {
                Ok(s) => s,
                Err(e) => {
                    error!("Storage lock poisoned: {e}");
                    return;
                }
            };
            if let Some(table_rows) = storage.tables.get(table_name) {
                if let Err(e) = persistence.save_table_data(table_name, table_rows) {
                    error!("Failed to save table data for {table_name}: {e}");
                }
            }
        }
    }

    fn execute_create_table(
        &self,
        db: &str,
        ct: &sqlparser::ast::CreateTable,
    ) -> Result<ExecuteResult, String> {
        let table_name = name_to_string(&ct.name);

        let mut catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let cols: Vec<Column> = ct
            .columns
            .iter()
            .map(|col| {
                let col_type = sql_type_to_column_type(&col.data_type);
                Column::new(&col.name.value, &table_name, col_type)
            })
            .collect();

        let wal_entry = WalEntry::CreateTable {
            table_name: table_name.clone(),
            columns: cols.clone(),
        };
        self.log_wal(&wal_entry);

        catalog.create_table(db, &table_name, cols)?;
        drop(catalog);
        self.save();
        info!("Created table {table_name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_insert(
        &self,
        db: &str,
        table_name: &str,
        columns: &[Ident],
        source: &Option<Box<Query>>,
    ) -> Result<ExecuteResult, String> {
        let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog.get_table(db, table_name)?.clone();
        drop(catalog);

        let query = source
            .as_ref()
            .ok_or_else(|| "INSERT missing source".to_string())?;

        let rows = match &*query.body {
            SetExpr::Values(values) => {
                let mut result = Vec::new();
                for row in &values.rows {
                    let vals: Vec<Value> = row
                        .iter()
                        .map(|expr| sql_expr_to_value(expr))
                        .collect();
                    if columns.is_empty() {
                        result.push(Row { values: vals });
                    } else {
                        let mut mapped = vec![Value::Null; table_def.columns.len()];
                        for (i, col_name) in columns.iter().enumerate() {
                            if let Some(val) = vals.get(i) {
                                let pos = table_def.columns
                                    .iter()
                                    .position(|c| c.name.eq_ignore_ascii_case(&col_name.value))
                                    .ok_or_else(|| format!("Unknown column: {}", col_name.value))?;
                                mapped[pos] = val.clone();
                            }
                        }
                        result.push(Row { values: mapped });
                    }
                }
                result
            }
            _ => return Err("INSERT only supports VALUES".into()),
        };

        let row_count = rows.len() as u64;
        let mut catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let first_id = catalog.next_row_id(table_name, row_count);
        drop(catalog);

        let wal_entry = WalEntry::InsertRows {
            table_name: table_name.to_string(),
            rows: rows.clone(),
        };
        self.log_wal(&wal_entry);

        let mut storage = self.storage.lock().map_err(|e| format!("Lock error: {e}"))?;
        storage.insert_rows(table_name, rows);
        drop(storage);
        self.save_table(table_name);

        info!("Inserted {row_count} rows into {table_name}");
        Ok(ExecuteResult::Affected {
            rows: row_count,
            last_insert_id: first_id,
        })
    }

    fn execute_delete(
        &self,
        db: &str,
        table_name: &str,
        selection: &Option<Expr>,
    ) -> Result<ExecuteResult, String> {
        let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog.get_table(db, table_name)?.clone();
        let table_columns = table_def.columns.clone();
        drop(catalog);

        let mut storage = self.storage.lock().map_err(|e| format!("Lock error: {e}"))?;
        let rows = storage.get_rows_mut(table_name)
            .ok_or_else(|| format!("Unknown table: {table_name}"))?;

        let before = rows.len();
        if let Some(expr) = selection {
            rows.retain(|row| {
                !eval_where(expr, &table_columns, &row.values).unwrap_or(false)
            });
        } else {
            rows.clear();
        }
        let deleted = (before - rows.len()) as u64;

        if deleted > 0 {
            let wal_entry = WalEntry::TableSnapshot {
                table_name: table_name.to_string(),
                rows: rows.clone(),
            };
            self.log_wal(&wal_entry);
        }

        drop(storage);
        self.save_table(table_name);

        info!("Deleted {deleted} rows from {table_name}");
        Ok(ExecuteResult::Affected {
            rows: deleted,
            last_insert_id: 0,
        })
    }

    fn execute_update(
        &self,
        db: &str,
        table_name: &str,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<Expr>,
    ) -> Result<ExecuteResult, String> {
        let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog.get_table(db, table_name)?.clone();
        let table_columns = table_def.columns.clone();
        drop(catalog);

        let assign_indices: Vec<(usize, Expr)> = {
            let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
            let table_def = catalog.get_table(db, table_name)?;
            let mut pairs = Vec::new();
            for assign in assignments {
                let col_name = match &assign.target {
                    AssignmentTarget::ColumnName(name) => name_to_string(name),
                    AssignmentTarget::Tuple(_) => {
                        return Err("Tuple assignment not supported".to_string());
                    }
                };
                let idx = table_def.columns.iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&col_name))
                    .ok_or_else(|| format!("Unknown column: {col_name}"))?;
                pairs.push((idx, assign.value.clone()));
            }
            pairs
        };

        let mut storage = self.storage.lock().map_err(|e| format!("Lock error: {e}"))?;
        let rows = storage.get_rows_mut(table_name)
            .ok_or_else(|| format!("Unknown table: {table_name}"))?;

        let mut updated = 0u64;
        for row in rows.iter_mut() {
            let matches = match selection {
                Some(expr) => eval_where(expr, &table_columns, &row.values).unwrap_or(false),
                None => true,
            };
            if matches {
                for (idx, expr) in &assign_indices {
                    if *idx < row.values.len() {
                        row.values[*idx] = sql_expr_to_value(expr);
                    }
                }
                updated += 1;
            }
        }

        if updated > 0 {
            let wal_entry = WalEntry::TableSnapshot {
                table_name: table_name.to_string(),
                rows: rows.clone(),
            };
            self.log_wal(&wal_entry);
        }

        drop(storage);
        self.save_table(table_name);

        info!("Updated {updated} rows in {table_name}");
        Ok(ExecuteResult::Affected {
            rows: updated,
            last_insert_id: 0,
        })
    }

    fn execute_query(&self, db: &str, query: &Query) -> Result<ExecuteResult, String> {
        match &*query.body {
            SetExpr::Select(select) => {
                let from_tables: Vec<String> = select
                    .from
                    .iter()
                    .map(|t| table_factor_name(&t.relation))
                    .collect();

                if from_tables.is_empty() {
                    return Ok(ExecuteResult::Rows {
                        columns: vec![],
                        rows: vec![],
                    });
                }

                let table_name = from_tables[0].as_str();

                let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
                let table_def = catalog.get_table(db, table_name)?.clone();
                let table_columns = table_def.columns.clone();
                drop(catalog);

                let cols: Vec<Column> = select
                    .projection
                    .iter()
                    .flat_map(|item| match item {
                        SelectItem::Wildcard(_) => table_columns.clone(),
                        SelectItem::UnnamedExpr(Expr::Wildcard(_)) => table_columns.clone(),
                        SelectItem::UnnamedExpr(Expr::Identifier(id)) => {
                            vec![find_column(&table_columns, &id.value)]
                        }
                        SelectItem::UnnamedExpr(Expr::Value(_)) => {
                            vec![Column::new("?", table_name, ColumnType::VarChar)]
                        }
                        _ => vec![Column::new("?", table_name, ColumnType::VarChar)],
                    })
                    .collect();

                let mut projected: Vec<Row> = {
                    let storage = self.storage.lock().map_err(|e| format!("Lock error: {e}"))?;
                    let all_rows = storage.get_rows(table_name);

                    let mut filtered: Vec<&Row> = if let Some(selection) = &select.selection {
                        all_rows
                            .into_iter()
                            .filter(|row| {
                                eval_where(selection, &table_columns, &row.values)
                                    .unwrap_or(false)
                            })
                            .collect()
                    } else {
                        all_rows
                    };

                    if let Some(order_by) = &query.order_by {
                        use sqlparser::ast::OrderByKind;
                        if let OrderByKind::Expressions(exprs) = &order_by.kind {
                            if !exprs.is_empty() {
                                sort_rows_refs(&mut filtered, exprs, &table_columns);
                            }
                        }
                    }

                    filtered
                        .into_iter()
                        .map(|row| {
                            let vals =
                                project_row(&select.projection, &table_columns, &row.values);
                            Row { values: vals }
                        })
                        .collect()
                };

                let offset = query.offset.as_ref().map(|o| offset_value(&o.value)).unwrap_or(0);
                let limit = query.limit.as_ref().map(|e| limit_value(e));

                if offset > 0 {
                    let start = offset.min(projected.len());
                    projected = projected.split_off(start);
                }

                if let Some(limit) = limit {
                    projected.truncate(limit);
                }

                Ok(ExecuteResult::Rows {
                    columns: cols,
                    rows: projected,
                })
            }
            _ => Err("Unsupported query type".into()),
        }
    }

    fn table_exists(&self, db: &str, table_name: &str) -> bool {
        self.catalog.lock()
            .map(|c| c.table_exists(db, table_name))
            .unwrap_or(false)
    }

    pub fn database_exists(&self, db_name: &str) -> bool {
        self.catalog.lock()
            .map(|c| c.databases.contains_key(db_name))
            .unwrap_or(false)
    }

    fn execute_drop_table(&self, db: &str, table_name: &str) -> Result<ExecuteResult, String> {
        let wal_entry = WalEntry::DropTable {
            table_name: table_name.to_string(),
        };
        self.log_wal(&wal_entry);

        let mut catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        catalog.drop_table(db, table_name)?;
        let mut storage = self.storage.lock().map_err(|e| format!("Lock error: {e}"))?;
        storage.clear_table(table_name);
        drop((catalog, storage));

        if let Some(ref persistence) = self.persistence {
            let _ = persistence.remove_table_data(table_name);
            let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
            let _ = persistence.save_catalog(&catalog);
        }

        info!("Dropped table {table_name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_create_database(&self, db_name: &ObjectName) -> Result<ExecuteResult, String> {
        let name = name_to_string(db_name);
        let mut catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        catalog.create_database(&name);
        drop(catalog);
        self.save();
        info!("Created database {name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_drop_database(&self, db_name: &str) -> Result<ExecuteResult, String> {
        let mut catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        catalog.databases.remove(db_name)
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;
        drop(catalog);
        self.save();
        info!("Dropped database {db_name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_show_tables(&self, db: &str) -> Result<ExecuteResult, String> {
        let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let db_def = catalog.databases.get(db)
            .ok_or_else(|| format!("Unknown database: {db}"))?;
        let mut names: Vec<String> = db_def.tables.keys().cloned().collect();
        names.sort();
        let column = Column::new(&format!("Tables_in_{db}"), "", ColumnType::VarChar);
        let rows: Vec<Row> = names
            .into_iter()
            .map(|name| Row {
                values: vec![Value::Text(name)],
            })
            .collect();
        Ok(ExecuteResult::Rows {
            columns: vec![column],
            rows,
        })
    }

    fn execute_show_databases(&self) -> Result<ExecuteResult, String> {
        let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        let mut names: Vec<String> = catalog.databases.keys().cloned().collect();
        names.sort();
        let column = Column::new("Database", "", ColumnType::VarChar);
        let rows: Vec<Row> = names
            .into_iter()
            .map(|name| Row {
                values: vec![Value::Text(name)],
            })
            .collect();
        Ok(ExecuteResult::Rows {
            columns: vec![column],
            rows,
        })
    }

    fn execute_use(&self, use_stmt: &SqlUse) -> Result<ExecuteResult, String> {
        let db_name = match use_stmt {
            SqlUse::Database(name) | SqlUse::Object(name) => name_to_string(name),
            _ => return Err("USE only supports database switching".into()),
        };

        let catalog = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
        if !catalog.databases.contains_key(&db_name) {
            return Err(format!("Unknown database: {db_name}"));
        }

        Ok(ExecuteResult::DatabaseChanged(db_name))
    }
}

fn name_to_string(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => ident.value.clone(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn table_factor_name(tf: &sqlparser::ast::TableFactor) -> String {
    match tf {
        sqlparser::ast::TableFactor::Table { name, .. } => name_to_string(name),
        _ => String::new(),
    }
}

fn table_object_name(to: &sqlparser::ast::TableObject) -> String {
    match to {
        sqlparser::ast::TableObject::TableName(name) => name_to_string(name),
        _ => String::new(),
    }
}

fn sql_type_to_column_type(dt: &DataType) -> ColumnType {
    match dt {
        DataType::Int(_) | DataType::Integer(_) => ColumnType::Long,
        DataType::BigInt(_) => ColumnType::LongLong,
        DataType::SmallInt(_) => ColumnType::Short,
        DataType::TinyInt(_) => ColumnType::Tiny,
        DataType::Float(_) => ColumnType::Float,
        DataType::Double(_) => ColumnType::Double,
        DataType::Text => ColumnType::Blob,
        DataType::Varchar(_) => ColumnType::VarChar,
        DataType::Char(_) => ColumnType::String,
        DataType::Boolean => ColumnType::Tiny,
        DataType::Blob(_) => ColumnType::Blob,
        DataType::Date => ColumnType::Date,
        DataType::Datetime(_) => ColumnType::DateTime,
        DataType::Timestamp { .. } => ColumnType::Timestamp,
        DataType::JSON => ColumnType::Json,
        _ => ColumnType::VarString,
    }
}

fn sql_expr_to_value(expr: &Expr) -> Value {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(n, _) => {
                if let Ok(v) = n.parse::<i64>() {
                    Value::Int(v)
                } else if let Ok(v) = n.parse::<f64>() {
                    Value::Double(v)
                } else {
                    Value::Text(n.clone())
                }
            }
            SqlValue::SingleQuotedString(s) => Value::Text(s.clone()),
            SqlValue::Null => Value::Null,
            _ => Value::Text(format!("{expr:?}")),
        },
        _ => Value::Text(format!("{expr:?}")),
    }
}

fn find_column(columns: &[Column], name: &str) -> Column {
    columns
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| Column::new(name, "", ColumnType::VarChar))
}

fn project_row(
    projection: &[SelectItem],
    table_columns: &[Column],
    values: &[Value],
) -> Vec<Value> {
    projection
        .iter()
        .flat_map(|item| match item {
            SelectItem::Wildcard(_) => values.to_vec(),
            SelectItem::UnnamedExpr(Expr::Wildcard(_)) => values.to_vec(),
            SelectItem::UnnamedExpr(Expr::Identifier(id)) => {
                let idx = table_columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&id.value));
                match idx {
                    Some(i) if i < values.len() => vec![values[i].clone()],
                    _ => vec![Value::Null],
                }
            }
            SelectItem::UnnamedExpr(Expr::Value(_)) => {
                vec![Value::Text("?".to_string())]
            }
            _ => vec![Value::Null],
        })
        .collect()
}

fn sort_rows_refs(rows: &mut Vec<&Row>, order_by: &[OrderByExpr], table_columns: &[Column]) {
    for ob in order_by.iter().rev() {
        let col_name = match &ob.expr {
            Expr::Identifier(id) => id.value.clone(),
            _ => continue,
        };
        let col_idx = table_columns.iter().position(|c| c.name.eq_ignore_ascii_case(&col_name));
        let Some(idx) = col_idx else { continue };
        let ascending = ob.options.asc.unwrap_or(true);

        rows.sort_by(|a, b| {
            let av = a.values.get(idx).cloned().unwrap_or(Value::Null);
            let bv = b.values.get(idx).cloned().unwrap_or(Value::Null);
            match value_cmp(&av, &bv) {
                Some(std::cmp::Ordering::Less) => {
                    if ascending { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater }
                }
                Some(std::cmp::Ordering::Greater) => {
                    if ascending { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less }
                }
                _ => std::cmp::Ordering::Equal,
            }
        });
    }
}

fn limit_value(expr: &Expr) -> usize {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(n, _) => n.parse().unwrap_or(0),
            _ => 0,
        },
        _ => 0,
    }
}

fn offset_value(expr: &Expr) -> usize {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(n, _) => n.parse().unwrap_or(0),
            _ => 0,
        },
        _ => 0,
    }
}

fn eval_where(expr: &Expr, columns: &[Column], values: &[Value]) -> Result<bool, String> {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            let lv = eval_expr(left, columns, values)?;
            let rv = eval_expr(right, columns, values)?;

            let result = match op {
                BinaryOperator::Eq => values_equal(&lv, &rv),
                BinaryOperator::NotEq => !values_equal(&lv, &rv),
                BinaryOperator::Gt => value_cmp(&lv, &rv) == Some(std::cmp::Ordering::Greater),
                BinaryOperator::GtEq => {
                    matches!(
                        value_cmp(&lv, &rv),
                        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                    )
                }
                BinaryOperator::Lt => value_cmp(&lv, &rv) == Some(std::cmp::Ordering::Less),
                BinaryOperator::LtEq => {
                    matches!(
                        value_cmp(&lv, &rv),
                        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                    )
                }
                BinaryOperator::And => {
                    let lb = as_bool(&lv).ok_or_else(|| "AND needs bool".to_string())?;
                    let rb = as_bool(&rv).ok_or_else(|| "AND needs bool".to_string())?;
                    lb && rb
                }
                BinaryOperator::Or => {
                    let lb = as_bool(&lv).ok_or_else(|| "OR needs bool".to_string())?;
                    let rb = as_bool(&rv).ok_or_else(|| "OR needs bool".to_string())?;
                    lb || rb
                }
                _ => return Err(format!("Unsupported operator: {op:?}")),
            };

            Ok(result)
        }
        Expr::Like { negated, expr, pattern, .. } => {
            let lv = eval_expr(expr, columns, values)?;
            let rv = eval_expr(pattern, columns, values)?;
            let result = like_match(&lv, &rv);
            Ok(if *negated { !result } else { result })
        }
        Expr::IsNull(expr) => {
            let v = eval_expr(expr, columns, values)?;
            Ok(matches!(v, Value::Null))
        }
        Expr::IsNotNull(expr) => {
            let v = eval_expr(expr, columns, values)?;
            Ok(!matches!(v, Value::Null))
        }
        Expr::Between { expr: inner, negated, low, high } => {
            let v = eval_expr(inner, columns, values)?;
            let lv = eval_expr(low, columns, values)?;
            let hv = eval_expr(high, columns, values)?;
            let between = match (value_cmp(&v, &lv), value_cmp(&v, &hv)) {
                (Some(a), Some(b)) => {
                    a != std::cmp::Ordering::Less && b != std::cmp::Ordering::Greater
                }
                _ => false,
            };
            Ok(if *negated { !between } else { between })
        }
        Expr::InList { expr: inner, list, negated } => {
            let v = eval_expr(inner, columns, values)?;
            let found = list.iter().any(|item| {
                if let Ok(iv) = eval_expr(item, columns, values) {
                    values_equal(&v, &iv)
                } else {
                    false
                }
            });
            Ok(if *negated { !found } else { found })
        }
        _ => {
            let v = eval_expr(expr, columns, values)?;
            Ok(as_bool(&v).unwrap_or(false))
        }
    }
}

fn eval_expr(expr: &Expr, columns: &[Column], values: &[Value]) -> Result<Value, String> {
    match expr {
        Expr::Identifier(id) => {
            let idx = columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&id.value));
            match idx {
                Some(i) if i < values.len() => Ok(values[i].clone()),
                _ => Ok(Value::Null),
            }
        }
        Expr::Value(_) => Ok(sql_expr_to_value(expr)),
        _ => Err(format!("Cannot evaluate expression: {expr:?}")),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::UInt(a), Value::UInt(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => (a - b).abs() < f32::EPSILON,
        (Value::Double(a), Value::Double(b)) => (a - b).abs() < f64::EPSILON,
        (Value::Text(a), Value::Text(b)) => a == b,
        (Value::Text(a), Value::Bytes(b)) => a.as_bytes() == b.as_slice(),
        (Value::Bytes(a), Value::Text(b)) => a.as_slice() == b.as_bytes(),
        (Value::Int(a), Value::Text(b)) => a.to_string() == *b,
        (Value::Text(a), Value::Int(b)) => *a == b.to_string(),
        (Value::Int(a), Value::Float(b)) => *a as f32 == *b,
        (Value::Float(a), Value::Int(b)) => *a == *b as f32,
        (Value::Int(a), Value::Double(b)) => (*a as f64 - b).abs() < f64::EPSILON,
        (Value::Double(a), Value::Int(b)) => (a - *b as f64).abs() < f64::EPSILON,
        (Value::Null, _) | (_, Value::Null) => false,
        _ => a.to_string() == b.to_string(),
    }
}

fn value_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::UInt(a), Value::UInt(b)) => Some(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Double(a), Value::Double(b)) => a.partial_cmp(b),
        (Value::Int(a), Value::Double(b)) => (*a as f64).partial_cmp(b),
        (Value::Double(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Int(a), Value::Float(b)) => (*a as f32).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f32)),
        (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::Text(b)) => Some(a.to_string().cmp(b)),
        (Value::Text(a), Value::Int(b)) => Some(a.cmp(&b.to_string())),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Int(n) => Some(*n != 0),
        Value::UInt(n) => Some(*n != 0),
        Value::Text(s) => Some(!s.is_empty() && s != "0"),
        _ => None,
    }
}

fn like_match(text: &Value, pattern: &Value) -> bool {
    let text_str = match text {
        Value::Text(s) => s.as_str(),
        Value::Null => return false,
        _ => return false,
    };
    let pattern_str = match pattern {
        Value::Text(s) => s.as_str(),
        Value::Null => return false,
        _ => return false,
    };

    let text_upper = text_str.to_uppercase();
    let pattern_upper = pattern_str.to_uppercase();

    like_match_impl(&text_upper, &pattern_upper)
}

fn like_match_impl(text: &str, pattern: &str) -> bool {
    let mut ti = 0;
    let mut pi = 0;
    let t = text.as_bytes();
    let p = pattern.as_bytes();

    while pi < p.len() {
        match p[pi] {
            b'%' => {
                pi += 1;
                if pi == p.len() {
                    return true;
                }
                while ti < t.len() {
                    if like_match_impl(&text[ti..], &pattern[pi..]) {
                        return true;
                    }
                    ti += 1;
                }
                return false;
            }
            b'_' => {
                if ti >= t.len() {
                    return false;
                }
                ti += 1;
                pi += 1;
            }
            c => {
                if ti >= t.len() || t[ti] != c {
                    return false;
                }
                ti += 1;
                pi += 1;
            }
        }
    }
    ti == t.len()
}
