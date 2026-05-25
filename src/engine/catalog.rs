use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::engine::index::IndexDef;
use crate::engine::types::Column;

fn normalize(name: &str) -> String {
    name.to_lowercase()
}

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
    pub indexes: HashMap<String, Vec<IndexDef>>,
}

impl Catalog {
    pub fn new() -> Self {
        let mut databases = HashMap::new();
        databases.insert(
            normalize("srrdb"),
            DatabaseDef {
                name: "srrdb".to_string(),
                tables: HashMap::new(),
                indexes: HashMap::new(),
            },
        );
        Catalog {
            databases,
            sequences: HashMap::new(),
        }
    }

    pub fn get_database(&self, name: &str) -> Option<&DatabaseDef> {
        self.databases.get(&normalize(name))
    }

    pub fn get_database_mut(&mut self, name: &str) -> Option<&mut DatabaseDef> {
        self.databases.get_mut(&normalize(name))
    }

    pub fn create_database(&mut self, name: &str) {
        self.databases
            .entry(normalize(name))
            .or_insert(DatabaseDef {
                name: name.to_string(),
                tables: HashMap::new(),
                indexes: HashMap::new(),
            });
    }

    pub fn database_exists(&self, name: &str) -> bool {
        self.databases.contains_key(&normalize(name))
    }

    pub fn remove_database(&mut self, name: &str) -> Option<DatabaseDef> {
        self.databases.remove(&normalize(name))
    }

    pub fn create_table(
        &mut self,
        db_name: &str,
        table_name: &str,
        columns: Vec<Column>,
    ) -> Result<(), String> {
        let tn = normalize(table_name);
        let db = self
            .databases
            .get_mut(&normalize(db_name))
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;

        if db.tables.contains_key(&tn) {
            return Err(format!("Table '{table_name}' already exists"));
        }

        db.tables.insert(
            tn.clone(),
            TableDef {
                name: table_name.to_string(),
                columns,
            },
        );
        db.indexes.entry(tn.clone()).or_default();
        self.sequences.insert(tn, 1);
        Ok(())
    }

    pub fn drop_table(&mut self, db_name: &str, table_name: &str) -> Result<(), String> {
        let tn = normalize(table_name);
        let db = self
            .databases
            .get_mut(&normalize(db_name))
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;

        db.tables
            .remove(&tn)
            .ok_or_else(|| format!("Unknown table: {table_name}"))?;
        db.indexes.remove(&tn);
        self.sequences.remove(&tn);
        Ok(())
    }

    pub fn get_table(&self, db_name: &str, table_name: &str) -> Result<&TableDef, String> {
        let db = self
            .databases
            .get(&normalize(db_name))
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;
        db.tables
            .get(&normalize(table_name))
            .ok_or_else(|| format!("Unknown table: {table_name}"))
    }

    pub fn next_row_id(&mut self, table_name: &str, count: u64) -> u64 {
        let tn = normalize(table_name);
        let current = self.sequences.get(&tn).copied().unwrap_or(1);
        self.sequences.insert(tn, current + count);
        current
    }

    pub fn table_exists(&self, db_name: &str, table_name: &str) -> bool {
        self.databases
            .get(&normalize(db_name))
            .and_then(|db| db.tables.get(&normalize(table_name)))
            .is_some()
    }

    pub fn create_index(
        &mut self,
        db_name: &str,
        index_def: IndexDef,
    ) -> Result<(), String> {
        let tn = normalize(&index_def.table_name);
        let db = self
            .databases
            .get_mut(&normalize(db_name))
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;

        if !db.tables.contains_key(&tn) {
            return Err(format!("Unknown table: {}", index_def.table_name));
        }

        let indexes = db.indexes.entry(tn).or_default();
        if indexes.iter().any(|i| i.name.eq_ignore_ascii_case(&index_def.name)) {
            return Err(format!("Index '{}' already exists", index_def.name));
        }

        indexes.push(index_def);
        Ok(())
    }

    pub fn drop_index(
        &mut self,
        db_name: &str,
        index_name: &str,
        table_name: &str,
    ) -> Result<IndexDef, String> {
        let tn = normalize(table_name);
        let db = self
            .databases
            .get_mut(&normalize(db_name))
            .ok_or_else(|| format!("Unknown database: {db_name}"))?;

        let indexes = db
            .indexes
            .get_mut(&tn)
            .ok_or_else(|| format!("No indexes on table '{table_name}'"))?;

        let pos = indexes
            .iter()
            .position(|i| i.name.eq_ignore_ascii_case(index_name))
            .ok_or_else(|| format!("Unknown index: {index_name}"))?;

        Ok(indexes.remove(pos))
    }

    pub fn get_table_indexes(&self, db_name: &str, table_name: &str) -> Vec<IndexDef> {
        let tn = normalize(table_name);
        self.databases
            .get(&normalize(db_name))
            .and_then(|db| db.indexes.get(&tn))
            .cloned()
            .unwrap_or_default()
    }

    pub fn get_index(&self, db_name: &str, table_name: &str, index_name: &str) -> Option<IndexDef> {
        let tn = normalize(table_name);
        self.databases
            .get(&normalize(db_name))
            .and_then(|db| db.indexes.get(&tn))
            .and_then(|indexes| {
                indexes
                    .iter()
                    .find(|i| i.name.eq_ignore_ascii_case(index_name))
                    .cloned()
            })
    }
}
