use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::engine::types::Value;

fn normalize(name: &str) -> String {
    name.to_lowercase()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Storage {
    pub tables: HashMap<String, Vec<Row>>,
}

impl Storage {
    pub fn new() -> Self {
        Storage {
            tables: HashMap::new(),
        }
    }

    pub fn insert_rows(&mut self, table_name: &str, rows: Vec<Row>) {
        let entry = self.tables.entry(normalize(table_name)).or_default();
        entry.extend(rows);
    }

    pub fn get_rows(&self, table_name: &str) -> Vec<&Row> {
        self.tables
            .get(&normalize(table_name))
            .map(|rows| rows.iter().collect())
            .unwrap_or_default()
    }

    pub fn get_rows_mut(&mut self, table_name: &str) -> Option<&mut Vec<Row>> {
        self.tables.get_mut(&normalize(table_name))
    }

    pub fn clear_table(&mut self, table_name: &str) {
        self.tables.remove(&normalize(table_name));
    }
}
