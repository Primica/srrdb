use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, ColumnOption, DataType, Expr, FromTable, Ident,
    ObjectName, ObjectNamePart, ObjectType, OrderByExpr, Query, SelectItem, SetExpr,
    Statement, TableFactor, Value as SqlValue,
};
use sqlparser::ast::Use as SqlUse;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::Token;
use tracing::{error, info};

use crate::engine::catalog::Catalog;
use crate::engine::index::{IndexData, IndexDef, IndexType};
use crate::engine::persistence::Persistence;
use crate::engine::storage::{Row, Storage};
use crate::engine::types::{Column, ColumnType, DefaultExpr, Value};
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
        Executor {
            catalog,
            storage,
            persistence: None,
            wal,
        }
    }

    pub fn with_persistence(
        catalog: Arc<Mutex<Catalog>>,
        storage: Arc<Mutex<Storage>>,
        persistence: Persistence,
    ) -> Self {
        let wal = Arc::new(Mutex::new(Wal::new(persistence.data_dir())));
        Executor {
            catalog,
            storage,
            persistence: Some(persistence),
            wal,
        }
    }

    pub fn with_wal(
        catalog: Arc<Mutex<Catalog>>,
        storage: Arc<Mutex<Storage>>,
        persistence: Persistence,
        wal: Arc<Mutex<Wal>>,
    ) -> Self {
        Executor {
            catalog,
            storage,
            persistence: Some(persistence),
            wal,
        }
    }

    pub fn rebuild_indexes(&self) {
        let catalog = match self.catalog.lock() {
            Ok(c) => c,
            Err(e) => {
                error!("Catalog lock poisoned: {e}");
                return;
            }
        };
        let db_names: Vec<String> = catalog.databases.keys().cloned().collect();
        drop(catalog);

        for db_name in &db_names {
            let catalog = match self.catalog.lock() {
                Ok(c) => c,
                Err(e) => {
                    error!("Catalog lock poisoned: {e}");
                    return;
                }
            };
            let database = match catalog.get_database(db_name) {
                Some(d) => d.clone(),
                None => continue,
            };
            drop(catalog);

            for (table_name, table_def) in &database.tables {
                let indexes = database
                    .indexes
                    .get(table_name)
                    .cloned()
                    .unwrap_or_default();
                if indexes.is_empty() {
                    continue;
                }

                let mut storage = match self.storage.lock() {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Storage lock poisoned: {e}");
                        return;
                    }
                };

                let tn = table_name.to_lowercase();
                let table_rows: Vec<(u64, Vec<Value>)> = storage
                    .tables
                    .get(&tn)
                    .map(|t| {
                        t.values()
                            .map(|r| (r.id, r.values.clone()))
                            .collect()
                    })
                    .unwrap_or_default();

                for index_def in &indexes {
                    let mut data = IndexData::new(index_def.index_type);
                    for (row_id, row_vals) in &table_rows {
                        let key: Vec<Value> = index_def
                            .columns
                            .iter()
                            .map(|cname| {
                                table_def
                                    .columns
                                    .iter()
                                    .position(|c| c.name.eq_ignore_ascii_case(cname))
                                    .and_then(|pos| row_vals.get(pos).cloned())
                                    .unwrap_or(Value::Null)
                            })
                            .collect();
                        data.insert(&key, *row_id);
                    }
                    let index_key = format!("{}:{}", tn, index_def.name.to_lowercase());
                    storage.index_data.insert(index_key, data);
                }
            }
        }
        info!("Indexes rebuilt from existing data");
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
                let table = del
                    .tables
                    .first()
                    .map(name_to_string)
                    .or_else(|| {
                        match &del.from {
                            FromTable::WithFromKeyword(tables)
                            | FromTable::WithoutKeyword(tables) => tables.first().and_then(|t| {
                                match &t.relation {
                                    TableFactor::Table { name, .. } => {
                                        Some(name_to_string(name))
                                    }
                                    _ => None,
                                }
                            }),
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
            Statement::Drop {
                object_type,
                names,
                if_exists,
                ..
            } if *object_type == ObjectType::Index => {
                let index_name = names.first().map(name_to_string).unwrap_or_default();
                let db_obj = self.catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
                let table_name = self
                    .find_index_table(db, &index_name, &db_obj)
                    .ok_or_else(|| format!("Unknown index: {index_name}"))?;
                drop(db_obj);
                if *if_exists && !self.index_exists(db, &index_name, &table_name) {
                    return Ok(ExecuteResult::Ok);
                }
                self.execute_drop_index(db, &index_name, &table_name)
            }
            Statement::CreateDatabase {
                db_name, if_not_exists, ..
            } => {
                let name = name_to_string(db_name);
                if *if_not_exists && self.database_exists(&name) {
                    return Ok(ExecuteResult::Ok);
                }
                self.execute_create_database(db_name)
            }
            Statement::ShowDatabases { .. } => self.execute_show_databases(),
            Statement::ShowTables { .. } => self.execute_show_tables(db),
            Statement::Use(use_stmt) => self.execute_use(use_stmt),
            Statement::ExplainTable {
                table_name, ..
            } => self.execute_describe(db, &name_to_string(table_name)),
            Statement::CreateIndex(ci) => {
                let table_name = name_to_string(&ci.table_name);
                if !self.table_exists(db, &table_name) {
                    return Err(format!("Unknown table: {table_name}"));
                }
                self.execute_create_index(db, &table_name, ci)
            }
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
            let tn = table_name.to_lowercase();
            if let Some(table_rows) = storage.tables.get(&tn) {
                if let Err(e) = persistence.save_table_data(&tn, table_rows) {
                    error!("Failed to save table data for {table_name}: {e}");
                }
            }
            let prefix = format!("{}:", tn);
            for (key, data) in &storage.index_data {
                if key.starts_with(&prefix) {
                    if let Err(e) = persistence.save_index_data(key, data) {
                        error!("Failed to save index data for {key}: {e}");
                    }
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

        let mut catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let cols: Vec<Column> = ct
            .columns
            .iter()
            .map(|col| {
                let col_type = sql_type_to_column_type(&col.data_type);
                let mut c = Column::new(&col.name.value, &table_name, col_type);
                for opt in &col.options {
                    match &opt.option {
                        ColumnOption::DialectSpecific(tokens) => {
                            if tokens.iter().any(|t| {
                                matches!(t, Token::Word(w) if w.keyword == Keyword::AUTO_INCREMENT)
                            }) {
                                c.auto_increment = true;
                            }
                        }
                        ColumnOption::Default(expr) => {
                            c.default_expr = parse_default_expr(expr);
                        }
                        _ => {}
                    }
                }
                c
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
        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog.get_table(db, table_name)?.clone();
        drop(catalog);

        let query = source
            .as_ref()
            .ok_or_else(|| "INSERT missing source".to_string())?;

        let mut rows = match &*query.body {
            SetExpr::Values(values) => {
                let mut result = Vec::new();
                for row in &values.rows {
                    let vals: Vec<Value> =
                        row.iter().map(|expr| sql_expr_to_value(expr)).collect();
                    if columns.is_empty() {
                        result.push(Row {
                            id: 0,
                            values: vals,
                        });
                    } else {
                        let mut mapped = vec![Value::Null; table_def.columns.len()];
                        for (i, col_name) in columns.iter().enumerate() {
                            if let Some(val) = vals.get(i) {
                                let pos = table_def
                                    .columns
                                    .iter()
                                    .position(|c| c.name.eq_ignore_ascii_case(&col_name.value))
                                    .ok_or_else(|| format!("Unknown column: {}", col_name.value))?;
                                mapped[pos] = val.clone();
                            }
                        }
                        result.push(Row {
                            id: 0,
                            values: mapped,
                        });
                    }
                }
                result
            }
            _ => return Err("INSERT only supports VALUES".into()),
        };

        let row_count = rows.len() as u64;
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;

        let mut last_insert_id = 0u64;
        for row in &mut rows {
            for (idx, col) in table_def.columns.iter().enumerate() {
                if matches!(row.values[idx], Value::Null) {
                    if col.auto_increment {
                        let id = catalog.next_row_id(table_name, 1);
                        row.values[idx] = Value::Int(id as i64);
                        last_insert_id = id;
                    } else if let Some(ref expr) = col.default_expr {
                        row.values[idx] = eval_default_expr(expr);
                    }
                }
            }
        }
        drop(catalog);

        let wal_entry = WalEntry::InsertRows {
            table_name: table_name.to_string(),
            rows: rows.clone(),
        };
        self.log_wal(&wal_entry);

        let mut storage = self
            .storage
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let inserted_ids = storage.insert_rows(table_name, rows);
        drop(storage);
        self.save_table(table_name);

        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let table_indexes = catalog.get_table_indexes(db, table_name);
        let col_refs: Vec<Column> = table_def.columns.clone();
        drop(catalog);

        if !table_indexes.is_empty() && !inserted_ids.is_empty() {
            let mut storage = self
                .storage
                .lock()
                .map_err(|e| format!("Lock error: {e}"))?;
            let tn = table_name.to_lowercase();
            for row_id in &inserted_ids {
                let row_vals = storage
                    .tables
                    .get(&tn)
                    .and_then(|t| t.get(row_id))
                    .map(|r| r.values.clone())
                    .unwrap_or_default();
                for idx_def in &table_indexes {
                    let key: Vec<Value> = idx_def
                        .columns
                        .iter()
                        .map(|cname| {
                            col_refs
                                .iter()
                                .position(|c| c.name.eq_ignore_ascii_case(cname))
                                .and_then(|pos| row_vals.get(pos).cloned())
                                .unwrap_or(Value::Null)
                        })
                        .collect();
                    let index_key = format!("{}:{}", tn, idx_def.name.to_lowercase());
                    storage
                        .index_data
                        .entry(index_key)
                        .or_insert_with(|| IndexData::new(idx_def.index_type))
                        .insert(&key, *row_id);
                }
            }
            drop(storage);
        }

        info!("Inserted {row_count} rows into {table_name}");
        Ok(ExecuteResult::Affected {
            rows: row_count,
            last_insert_id,
        })
    }

    fn execute_delete(
        &self,
        db: &str,
        table_name: &str,
        selection: &Option<Expr>,
    ) -> Result<ExecuteResult, String> {
        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog.get_table(db, table_name)?.clone();
        let table_columns = table_def.columns.clone();
        let table_indexes = catalog.get_table_indexes(db, table_name);
        drop(catalog);

        let mut storage = self
            .storage
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;

        let (to_remove_ids, to_remove_vals) = {
            let table_data = storage
                .get_table_mut(table_name)
                .ok_or_else(|| format!("Unknown table: {table_name}"))?;

            if let Some(expr) = selection {
                let mut ids = Vec::new();
                let mut vals = Vec::new();
                for (id, row) in table_data.iter() {
                    if eval_where(expr, &table_columns, &row.values).unwrap_or(false) {
                        ids.push(*id);
                        vals.push(row.values.clone());
                    }
                }
                (ids, vals)
            } else {
                let ids: Vec<u64> = table_data.keys().copied().collect();
                let vals: Vec<Vec<Value>> =
                    table_data.values().map(|r| r.values.clone()).collect();
                (ids, vals)
            }
        };

        let deleted = to_remove_ids.len() as u64;

        for (i, id) in to_remove_ids.iter().enumerate() {
            let row_ref = Row {
                id: *id,
                values: to_remove_vals[i].clone(),
            };
            self.delete_from_indexes_raw(
                &mut storage.index_data,
                table_name,
                &table_indexes,
                &table_columns,
                &row_ref,
            );
        }

        // Remove rows from table
        if let Some(table_data) = storage.get_table_mut(table_name) {
            for id in &to_remove_ids {
                table_data.remove(id);
            }
        }

        if deleted > 0 {
            let remaining: Vec<Row> = storage
                .tables
                .get(&table_name.to_lowercase())
                .map(|t| t.values().cloned().collect())
                .unwrap_or_default();
            let wal_entry = WalEntry::TableSnapshot {
                table_name: table_name.to_string(),
                rows: remaining,
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
        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog.get_table(db, table_name)?.clone();
        let table_columns = table_def.columns.clone();
        let table_indexes = catalog.get_table_indexes(db, table_name);
        drop(catalog);

        let assign_indices: Vec<(usize, Expr)> = {
            let catalog = self
                .catalog
                .lock()
                .map_err(|e| format!("Lock error: {e}"))?;
            let table_def = catalog.get_table(db, table_name)?;
            let mut pairs = Vec::new();
            for assign in assignments {
                let col_name = match &assign.target {
                    AssignmentTarget::ColumnName(name) => name_to_string(name),
                    AssignmentTarget::Tuple(_) => {
                        return Err("Tuple assignment not supported".to_string());
                    }
                };
                let idx = table_def
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&col_name))
                    .ok_or_else(|| format!("Unknown column: {col_name}"))?;
                pairs.push((idx, assign.value.clone()));
            }
            pairs
        };

        let mut storage = self
            .storage
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;

        // Collect matching rows: (id, old_values snapshot)
        let to_update: Vec<(u64, Vec<Value>)> = {
            let table_data = storage
                .get_table_mut(table_name)
                .ok_or_else(|| format!("Unknown table: {table_name}"))?;

            table_data
                .iter()
                .filter(|(_, row)| {
                    selection
                        .as_ref()
                        .map(|expr| eval_where(expr, &table_columns, &row.values).unwrap_or(false))
                        .unwrap_or(true)
                })
                .map(|(id, row)| (*id, row.values.clone()))
                .collect()
        };

        let updated = to_update.len() as u64;

        // Delete old entries from indexes
        for (id, old_vals) in &to_update {
            let old_row = Row {
                id: *id,
                values: old_vals.clone(),
            };
            self.delete_from_indexes_raw(
                &mut storage.index_data,
                table_name,
                &table_indexes,
                &table_columns,
                &old_row,
            );
        }

        // Update row values and collect new snapshots
        let new_rows: Vec<(u64, Vec<Value>)> = to_update
            .into_iter()
            .map(|(id, mut vals)| {
                for (idx, expr) in &assign_indices {
                    if *idx < vals.len() {
                        vals[*idx] = sql_expr_to_value(expr);
                    }
                }
                (id, vals)
            })
            .collect();

        // Apply updates to table data
        if let Some(table_data) = storage.get_table_mut(table_name) {
            for (id, new_vals) in &new_rows {
                if let Some(row) = table_data.get_mut(id) {
                    row.values.clone_from(new_vals);
                }
            }
        }

        // Drop table_data borrow before inserting new index entries
        drop(storage.get_table_mut(table_name));

        // Insert new index entries
        for (id, new_vals) in &new_rows {
            let new_row = Row {
                id: *id,
                values: new_vals.clone(),
            };
            self.insert_into_indexes_raw(
                &mut storage.index_data,
                table_name,
                &table_indexes,
                &table_columns,
                &new_row,
            );
        }

        if updated > 0 {
            let remaining: Vec<Row> = storage
                .tables
                .get(&table_name.to_lowercase())
                .map(|t| t.values().cloned().collect())
                .unwrap_or_default();
            let wal_entry = WalEntry::TableSnapshot {
                table_name: table_name.to_string(),
                rows: remaining,
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

                let catalog = self
                    .catalog
                    .lock()
                    .map_err(|e| format!("Lock error: {e}"))?;
                let table_def = catalog.get_table(db, table_name)?.clone();
                let table_columns = table_def.columns.clone();
                let table_indexes = catalog.get_table_indexes(db, table_name);
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

                let all_rows: Vec<Row> = {
                    let storage = self
                        .storage
                        .lock()
                        .map_err(|e| format!("Lock error: {e}"))?;

                    let indexed_ids = if let Some(selection) = &select.selection {
                        self.try_index_lookup(
                            &storage,
                            table_name,
                            selection,
                            &table_columns,
                            &table_indexes,
                        )
                    } else {
                        None
                    };

                    if let Some(ids) = indexed_ids {
                        let table_data = storage
                            .tables
                            .get(&table_name.to_lowercase())
                            .cloned()
                            .unwrap_or_default();
                        drop(storage);
                        let mut rows: Vec<Row> = ids
                            .iter()
                            .filter_map(|id| table_data.get(id).cloned())
                            .collect();
                        if let Some(selection) = &select.selection {
                            rows.retain(|row| {
                                eval_where(selection, &table_columns, &row.values).unwrap_or(false)
                            });
                        }
                        rows
                    } else {
                        let rows: Vec<Row> = storage
                            .get_rows(table_name)
                            .into_iter()
                            .cloned()
                            .collect();
                        drop(storage);
                        if let Some(selection) = &select.selection {
                            let filtered: Vec<Row> = rows
                                .into_iter()
                                .filter(|row| {
                                    eval_where(selection, &table_columns, &row.values)
                                        .unwrap_or(false)
                                })
                                .collect();
                            filtered
                        } else {
                            rows
                        }
                    }
                };

                let mut projected: Vec<Row> = {
                    let mut filtered: Vec<Row> = all_rows;

                    if let Some(order_by) = &query.order_by {
                        use sqlparser::ast::OrderByKind;
                        if let OrderByKind::Expressions(exprs) = &order_by.kind {
                            if !exprs.is_empty() {
                                filtered = sort_rows(filtered, exprs, &table_columns);
                            }
                        }
                    }

                    filtered
                        .into_iter()
                        .map(|row| {
                            let vals =
                                project_row(&select.projection, &table_columns, &row.values);
                            Row {
                                id: 0,
                                values: vals,
                            }
                        })
                        .collect()
                };

                let offset = query
                    .offset
                    .as_ref()
                    .map(|o| offset_value(&o.value))
                    .unwrap_or(0);
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

    // ── INDEX LOOKUP ──────────────────────────────────────────

    fn try_index_lookup(
        &self,
        storage: &Storage,
        table_name: &str,
        selection: &Expr,
        table_columns: &[Column],
        indexes: &[IndexDef],
    ) -> Option<Vec<u64>> {
        if indexes.is_empty() {
            return None;
        }

        let eq_conditions = extract_eq_conditions(selection, table_columns);
        if eq_conditions.is_empty() {
            return None;
        }

        let tn = table_name.to_lowercase();

        for index_def in indexes {
            let mut key = Vec::new();
            let mut full_match = true;
            for col_name in &index_def.columns {
                if let Some(val) = eq_conditions.get(&col_name.to_lowercase()) {
                    key.push(val.clone());
                } else {
                    full_match = false;
                    break;
                }
            }
            if !full_match || key.is_empty() {
                continue;
            }

            let index_key = format!("{}:{}", tn, index_def.name.to_lowercase());
            if let Some(index_data) = storage.index_data.get(&index_key) {
                let ids = index_data.lookup_eq(&key);
                if !ids.is_empty() {
                    return Some(ids);
                }
                // If the index is unique but the key wasn't found, return empty
                if index_def.unique {
                    return Some(vec![]);
                }
                // If the index exists but no matches, still return the empty set
                return Some(ids);
            }
        }

        None
    }

    // ── INDEX MAINTENANCE ──────────────────────────────────────

    fn get_index_key(
        &self,
        row: &Row,
        index_def: &IndexDef,
        table_columns: &[Column],
    ) -> Vec<Value> {
        index_def
            .columns
            .iter()
            .map(|cname| {
                table_columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(cname))
                    .and_then(|pos| row.values.get(pos).cloned())
                    .unwrap_or(Value::Null)
            })
            .collect()
    }

    fn insert_into_indexes_raw(
        &self,
        index_data: &mut std::collections::HashMap<String, IndexData>,
        table_name: &str,
        indexes: &[IndexDef],
        table_columns: &[Column],
        row: &Row,
    ) {
        let tn = table_name.to_lowercase();
        for idx_def in indexes {
            let key = self.get_index_key(row, idx_def, table_columns);
            let index_key = format!("{}:{}", tn, idx_def.name.to_lowercase());
            index_data
                .entry(index_key)
                .or_insert_with(|| IndexData::new(idx_def.index_type))
                .insert(&key, row.id);
        }
    }

    fn delete_from_indexes_raw(
        &self,
        index_data: &mut std::collections::HashMap<String, IndexData>,
        table_name: &str,
        indexes: &[IndexDef],
        table_columns: &[Column],
        row: &Row,
    ) {
        let tn = table_name.to_lowercase();
        for idx_def in indexes {
            let key = self.get_index_key(row, idx_def, table_columns);
            let index_key = format!("{}:{}", tn, idx_def.name.to_lowercase());
            if let Some(data) = index_data.get_mut(&index_key) {
                data.delete(&key, row.id);
            }
        }
    }

    fn table_exists(&self, db: &str, table_name: &str) -> bool {
        self.catalog
            .lock()
            .map(|c| c.table_exists(db, table_name))
            .unwrap_or(false)
    }

    pub fn database_exists(&self, db_name: &str) -> bool {
        self.catalog
            .lock()
            .map(|c| c.database_exists(db_name))
            .unwrap_or(false)
    }

    fn index_exists(&self, db: &str, index_name: &str, table_name: &str) -> bool {
        self.catalog
            .lock()
            .map(|c| {
                c.get_table_indexes(db, table_name)
                    .iter()
                    .any(|i| i.name.eq_ignore_ascii_case(index_name))
            })
            .unwrap_or(false)
    }

    fn find_index_table(&self, db: &str, index_name: &str, catalog: &Catalog) -> Option<String> {
        if let Some(database) = catalog.get_database(db) {
            for (table_name, indexes) in &database.indexes {
                if indexes.iter().any(|i| i.name.eq_ignore_ascii_case(index_name)) {
                    return Some(table_name.clone());
                }
            }
        }
        None
    }

    fn execute_create_index(
        &self,
        db: &str,
        table_name: &str,
        ci: &sqlparser::ast::CreateIndex,
    ) -> Result<ExecuteResult, String> {
        let index_name = match &ci.name {
            Some(name) => name_to_string(name),
            None => {
                let col_names: Vec<String> = ci
                    .columns
                    .iter()
                    .filter_map(|o| match &o.expr {
                        Expr::Identifier(id) => Some(id.value.clone()),
                        _ => None,
                    })
                    .collect();
                format!("idx_{}_{}", table_name, col_names.join("_"))
            }
        };

        let index_type = match &ci.using {
            Some(ident) if ident.value.eq_ignore_ascii_case("hash") => IndexType::Hash,
            _ => IndexType::BTree,
        };

        let columns: Vec<String> = ci
            .columns
            .iter()
            .filter_map(|o| match &o.expr {
                Expr::Identifier(id) => Some(id.value.clone()),
                _ => None,
            })
            .collect();

        if columns.is_empty() {
            return Err("Index must have at least one column".to_string());
        }

        let index_def = IndexDef {
            name: index_name.clone(),
            index_type,
            table_name: table_name.to_string(),
            columns: columns.clone(),
            unique: ci.unique,
        };

        {
            let mut catalog = self
                .catalog
                .lock()
                .map_err(|e| format!("Lock error: {e}"))?;
            catalog.create_index(db, index_def.clone())?;
        }

        let wal_entry = WalEntry::CreateIndex {
            db_name: db.to_string(),
            index_def: index_def.clone(),
        };
        self.log_wal(&wal_entry);

        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let table_def = catalog
            .get_table(db, table_name)?
            .clone();
        let table_columns = table_def.columns.clone();
        drop(catalog);

        let mut storage = self
            .storage
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let mut index_data = IndexData::new(index_type);

        if let Some(table_data) = storage.tables.get(&table_name.to_lowercase()) {
            for row in table_data.values() {
                let key: Vec<Value> = columns
                    .iter()
                    .map(|cname| {
                        table_columns
                            .iter()
                            .position(|c| c.name.eq_ignore_ascii_case(cname))
                            .and_then(|pos| row.values.get(pos).cloned())
                            .unwrap_or(Value::Null)
                    })
                    .collect();

                if index_def.unique {
                    let existing = index_data.lookup_eq(&key);
                    if !existing.is_empty() {
                        // Remove the partially-built index
                        let index_key =
                            format!("{}:{}", table_name.to_lowercase(), index_name.to_lowercase());
                        storage.index_data.remove(&index_key);
                        return Err(format!(
                            "Duplicate entry '{}' for key '{}'",
                            key.iter()
                                .map(|v| v.to_string())
                                .collect::<Vec<_>>()
                                .join(","),
                            index_name
                        ));
                    }
                }

                index_data.insert(&key, row.id);
            }
        }

        let index_key = format!("{}:{}", table_name.to_lowercase(), index_name.to_lowercase());
        storage
            .index_data
            .insert(index_key.clone(), index_data);
        drop(storage);

        self.save_table(table_name);
        self.save();

        info!("Created index {index_name} on {table_name} ({})", index_type);
        Ok(ExecuteResult::Ok)
    }

    fn execute_drop_index(
        &self,
        db: &str,
        index_name: &str,
        table_name: &str,
    ) -> Result<ExecuteResult, String> {
        {
            let mut catalog = self
                .catalog
                .lock()
                .map_err(|e| format!("Lock error: {e}"))?;
            catalog.drop_index(db, index_name, table_name)?;
        }

        let wal_entry = WalEntry::DropIndex {
            db_name: db.to_string(),
            index_name: index_name.to_string(),
            table_name: table_name.to_string(),
        };
        self.log_wal(&wal_entry);

        let mut storage = self
            .storage
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let index_key = format!(
            "{}:{}",
            table_name.to_lowercase(),
            index_name.to_lowercase()
        );
        storage.index_data.remove(&index_key);
        drop(storage);

        if let Some(ref persistence) = self.persistence {
            let _ = persistence.remove_index_data(&index_key);
        }

        info!("Dropped index {index_name} from {table_name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_drop_table(
        &self,
        db: &str,
        table_name: &str,
    ) -> Result<ExecuteResult, String> {
        let tn = table_name.to_lowercase();
        let wal_entry = WalEntry::DropTable {
            table_name: tn.clone(),
        };
        self.log_wal(&wal_entry);

        let mut catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        catalog.drop_table(db, &tn)?;
        let mut storage = self
            .storage
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        storage.clear_table(&tn);
        drop((catalog, storage));

        if let Some(ref persistence) = self.persistence {
            let _ = persistence.remove_table_data(&tn);
            let catalog = self
                .catalog
                .lock()
                .map_err(|e| format!("Lock error: {e}"))?;
            let _ = persistence.save_catalog(&catalog);
            // Remove index data files
            if let Ok(dir) = std::fs::read_dir(persistence.data_dir().join("indexes")) {
                for entry in dir.flatten() {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if fname.starts_with(&tn) || fname.contains(&format!("_{}_", tn)) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }

        info!("Dropped table {table_name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_create_database(
        &self,
        db_name: &ObjectName,
    ) -> Result<ExecuteResult, String> {
        let name = name_to_string(db_name);
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        catalog.create_database(&name);
        drop(catalog);
        self.save();
        info!("Created database {name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_drop_database(&self, db_name: &str) -> Result<ExecuteResult, String> {
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        catalog
            .remove_database(db_name)
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;
        drop(catalog);
        self.save();
        info!("Dropped database {db_name}");
        Ok(ExecuteResult::Ok)
    }

    fn execute_show_tables(&self, db: &str) -> Result<ExecuteResult, String> {
        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let db_def = catalog
            .get_database(db)
            .ok_or_else(|| format!("Unknown database: {db}"))?;
        let mut names: Vec<String> = db_def.tables.keys().cloned().collect();
        names.sort();
        let column = Column::new(&format!("Tables_in_{db}"), "", ColumnType::VarChar);
        let rows: Vec<Row> = names
            .into_iter()
            .map(|name| Row {
                id: 0,
                values: vec![Value::Text(name)],
            })
            .collect();
        Ok(ExecuteResult::Rows {
            columns: vec![column],
            rows,
        })
    }

    fn execute_show_databases(&self) -> Result<ExecuteResult, String> {
        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let mut names: Vec<String> =
            catalog.databases.values().map(|d| d.name.clone()).collect();
        names.sort();
        let column = Column::new("Database", "", ColumnType::VarChar);
        let rows: Vec<Row> = names
            .into_iter()
            .map(|name| Row {
                id: 0,
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

        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        if !catalog.database_exists(&db_name) {
            return Err(format!("Unknown database: {db_name}"));
        }

        Ok(ExecuteResult::DatabaseChanged(db_name))
    }

    fn execute_describe(&self, db: &str, table_name: &str) -> Result<ExecuteResult, String> {
        let catalog = self
            .catalog
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        let table = catalog.get_table(db, table_name)?.clone();
        drop(catalog);

        let field_col = Column::new("Field", "", ColumnType::VarChar);
        let type_col = Column::new("Type", "", ColumnType::VarChar);
        let null_col = Column::new("Null", "", ColumnType::VarChar);
        let key_col = Column::new("Key", "", ColumnType::VarChar);
        let default_col = Column::new("Default", "", ColumnType::VarChar);
        let extra_col = Column::new("Extra", "", ColumnType::VarChar);

        let mut rows = Vec::new();
        for col in &table.columns {
            let type_str = column_type_name(col.col_type, col.column_length);
            let null_str = "YES".to_string();
            let key_str = if col.auto_increment {
                "PRI".to_string()
            } else {
                String::new()
            };
            let default_str = match &col.default_expr {
                Some(expr) => match expr {
                    DefaultExpr::Value(v) => v.to_string(),
                    DefaultExpr::CurrentTimestamp => "CURRENT_TIMESTAMP".to_string(),
                },
                None => String::new(),
            };
            let mut extra_parts = Vec::new();
            if col.auto_increment {
                extra_parts.push("auto_increment");
            }
            if col.default_expr.is_some() {
                extra_parts.push("DEFAULT");
            }
            let extra_str = extra_parts.join(" ");

            rows.push(Row {
                id: 0,
                values: vec![
                    Value::Text(col.name.clone()),
                    Value::Text(type_str),
                    Value::Text(null_str),
                    Value::Text(key_str),
                    if default_str.is_empty() {
                        Value::Null
                    } else {
                        Value::Text(default_str)
                    },
                    Value::Text(extra_str),
                ],
            });
        }

        Ok(ExecuteResult::Rows {
            columns: vec![field_col, type_col, null_col, key_col, default_col, extra_col],
            rows,
        })
    }
}

// ── helper functions ────────────────────────────────────────────

fn extract_eq_conditions(
    expr: &Expr,
    columns: &[Column],
) -> std::collections::HashMap<String, Value> {
    let mut conditions = std::collections::HashMap::new();
    extract_eq_conditions_recursive(expr, columns, &mut conditions);
    conditions
}

fn extract_eq_conditions_recursive(
    expr: &Expr,
    columns: &[Column],
    acc: &mut std::collections::HashMap<String, Value>,
) {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            extract_eq_conditions_recursive(left, columns, acc);
            extract_eq_conditions_recursive(right, columns, acc);
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } => {
            if let (Some(col_name), Some(val)) =
                (extract_column_ref(left, columns), eval_const_expr(right))
            {
                acc.entry(col_name.to_lowercase()).or_insert(val);
            } else if let (Some(col_name), Some(val)) =
                (extract_column_ref(right, columns), eval_const_expr(left))
            {
                acc.entry(col_name.to_lowercase()).or_insert(val);
            }
        }
        _ => {}
    }
}

fn extract_column_ref(expr: &Expr, _columns: &[Column]) -> Option<String> {
    match expr {
        Expr::Identifier(id) => Some(id.value.clone()),
        _ => None,
    }
}

fn eval_const_expr(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(n, _) => {
                if let Ok(v) = n.parse::<i64>() {
                    Some(Value::Int(v))
                } else if let Ok(v) = n.parse::<f64>() {
                    Some(Value::Double(v))
                } else {
                    Some(Value::Text(n.clone()))
                }
            }
            SqlValue::SingleQuotedString(s) => Some(Value::Text(s.clone())),
            SqlValue::Null => Some(Value::Null),
            _ => None,
        },
        _ => None,
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

fn column_type_name(ct: ColumnType, len: u32) -> String {
    let base = match ct {
        ColumnType::Decimal => "decimal",
        ColumnType::Tiny => "tinyint",
        ColumnType::Short => "smallint",
        ColumnType::Long => "int",
        ColumnType::Float => "float",
        ColumnType::Double => "double",
        ColumnType::Null => "null",
        ColumnType::Timestamp => "timestamp",
        ColumnType::LongLong => "bigint",
        ColumnType::Int24 => "mediumint",
        ColumnType::Date => "date",
        ColumnType::Time => "time",
        ColumnType::DateTime => "datetime",
        ColumnType::Year => "year",
        ColumnType::VarChar => "varchar",
        ColumnType::Bit => "bit",
        ColumnType::Json => "json",
        ColumnType::NewDecimal => "decimal",
        ColumnType::Enum => "enum",
        ColumnType::Set => "set",
        ColumnType::TinyBlob => "tinyblob",
        ColumnType::MediumBlob => "mediumblob",
        ColumnType::LongBlob => "longblob",
        ColumnType::Blob => "text",
        ColumnType::VarString => "varchar",
        ColumnType::String => "char",
        ColumnType::Geometry => "geometry",
    };
    match ct {
        ColumnType::VarChar | ColumnType::VarString | ColumnType::String => {
            format!("{base}({len})")
        }
        _ => base.to_string(),
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

fn sort_rows(
    mut rows: Vec<Row>,
    order_by: &[OrderByExpr],
    table_columns: &[Column],
) -> Vec<Row> {
    for ob in order_by.iter().rev() {
        let col_name = match &ob.expr {
            Expr::Identifier(id) => id.value.clone(),
            _ => continue,
        };
        let col_idx = table_columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&col_name));
        let Some(idx) = col_idx else {
            continue;
        };
        let ascending = ob.options.asc.unwrap_or(true);

        rows.sort_by(|a, b| {
            let av = a.values.get(idx).cloned().unwrap_or(Value::Null);
            let bv = b.values.get(idx).cloned().unwrap_or(Value::Null);
            match value_cmp(&av, &bv) {
                Some(std::cmp::Ordering::Less) => {
                    if ascending {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                Some(std::cmp::Ordering::Greater) => {
                    if ascending {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Less
                    }
                }
                _ => std::cmp::Ordering::Equal,
            }
        });
    }
    rows
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

fn eval_where(
    expr: &Expr,
    columns: &[Column],
    values: &[Value],
) -> Result<bool, String> {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            let lv = eval_expr(left, columns, values)?;
            let rv = eval_expr(right, columns, values)?;

            let result = match op {
                BinaryOperator::Eq => values_equal(&lv, &rv),
                BinaryOperator::NotEq => !values_equal(&lv, &rv),
                BinaryOperator::Gt => {
                    value_cmp(&lv, &rv) == Some(std::cmp::Ordering::Greater)
                }
                BinaryOperator::GtEq => matches!(
                    value_cmp(&lv, &rv),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                ),
                BinaryOperator::Lt => {
                    value_cmp(&lv, &rv) == Some(std::cmp::Ordering::Less)
                }
                BinaryOperator::LtEq => matches!(
                    value_cmp(&lv, &rv),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                ),
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
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        } => {
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
        Expr::Between {
            expr: inner,
            negated,
            low,
            high,
        } => {
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
        Expr::InList {
            expr: inner,
            list,
            negated,
        } => {
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

fn parse_default_expr(expr: &Expr) -> Option<DefaultExpr> {
    match expr {
        Expr::Value(_) => Some(DefaultExpr::Value(sql_expr_to_value(expr))),
        Expr::Function(f) => {
            let name = f.name.to_string();
            if name.eq_ignore_ascii_case("now")
                || name.eq_ignore_ascii_case("current_timestamp")
            {
                Some(DefaultExpr::CurrentTimestamp)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn eval_default_expr(expr: &DefaultExpr) -> Value {
    match expr {
        DefaultExpr::Value(v) => v.clone(),
        DefaultExpr::CurrentTimestamp => format_timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        ),
    }
}

fn format_timestamp(secs: u64) -> Value {
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let mut y = 1970i64;
    let mut d = days as i64;
    loop {
        let year_days = if is_leap(y) { 366 } else { 365 };
        if d < year_days {
            break;
        }
        d -= year_days;
        y += 1;
    }
    let month_days: &[i64] = if is_leap(y) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0i64;
    for (i, &md) in month_days.iter().enumerate() {
        if d < md {
            m = i as i64 + 1;
            break;
        }
        d -= md;
    }
    let day = d + 1;
    let h = (time_secs / 3600) as i64;
    let mi = ((time_secs % 3600) / 60) as i64;
    let s = (time_secs % 60) as i64;
    Value::Text(format!("{y:04}-{m:02}-{day:02} {h:02}:{mi:02}:{s:02}"))
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
