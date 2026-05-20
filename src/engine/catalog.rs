use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::engine::types::Column;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDef {
    pub name: String,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub databases: HashMap<String, DatabaseDef>,
    pub sequences: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseDef {
    pub name: String,
    pub tables: HashMap<String, TableDef>,
}

impl Catalog {
    pub fn new() -> Self {
        let mut databases = HashMap::new();
        databases.insert(
            "srrdb".to_string(),
            DatabaseDef {
                name: "srrdb".to_string(),
                tables: HashMap::new(),
            },
        );
        Catalog { databases, sequences: HashMap::new() }
    }

    pub fn get_database(&self, name: &str) -> Option<&DatabaseDef> {
        self.databases.get(name)
    }

    pub fn get_database_mut(&mut self, name: &str) -> Option<&mut DatabaseDef> {
        self.databases.get_mut(name)
    }

    pub fn create_database(&mut self, name: &str) {
        self.databases.entry(name.to_string()).or_insert(DatabaseDef {
            name: name.to_string(),
            tables: HashMap::new(),
        });
    }

    pub fn create_table(
        &mut self,
        db_name: &str,
        table_name: &str,
        columns: Vec<Column>,
    ) -> Result<(), String> {
        let db = self
            .databases
            .get_mut(db_name)
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;

        if db.tables.contains_key(table_name) {
            return Err(format!("Table '{table_name}' already exists"));
        }

        db.tables.insert(
            table_name.to_string(),
            TableDef {
                name: table_name.to_string(),
                columns,
            },
        );
        self.sequences.insert(table_name.to_string(), 1);
        Ok(())
    }

    pub fn drop_table(&mut self, db_name: &str, table_name: &str) -> Result<(), String> {
        let db = self
            .databases
            .get_mut(db_name)
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;

        db.tables
            .remove(table_name)
            .ok_or_else(|| format!("Unknown table: {table_name}"))?;
        self.sequences.remove(table_name);
        Ok(())
    }

    pub fn get_table(&self, db_name: &str, table_name: &str) -> Result<&TableDef, String> {
        let db = self
            .databases
            .get(db_name)
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;
        db.tables
            .get(table_name)
            .ok_or_else(|| format!("Unknown table: {table_name}"))
    }

    pub fn next_row_id(&mut self, table_name: &str, count: u64) -> u64 {
        let current = self.sequences.get(table_name).copied().unwrap_or(1);
        self.sequences.insert(table_name.to_string(), current + count);
        current
    }

    pub fn table_exists(&self, db_name: &str, table_name: &str) -> bool {
        self.databases
            .get(db_name)
            .and_then(|db| db.tables.get(table_name))
            .is_some()
    }
}
