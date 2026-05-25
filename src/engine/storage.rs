use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::engine::index::IndexData;
use crate::engine::types::Value;

fn normalize(name: &str) -> String {
    name.to_lowercase()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    #[serde(default)]
    pub id: u64,
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Storage {
    pub tables: HashMap<String, BTreeMap<u64, Row>>,
    pub next_ids: HashMap<String, u64>,
    pub index_data: HashMap<String, IndexData>,
}

impl Storage {
    pub fn new() -> Self {
        Storage {
            tables: HashMap::new(),
            next_ids: HashMap::new(),
            index_data: HashMap::new(),
        }
    }

    pub fn insert_rows(&mut self, table_name: &str, rows: Vec<Row>) -> Vec<u64> {
        let entry = self.tables.entry(normalize(table_name)).or_default();
        let next_id = self.next_ids.entry(normalize(table_name)).or_insert(1);
        let mut ids = Vec::with_capacity(rows.len());
        for mut row in rows {
            if row.id == 0 {
                row.id = *next_id;
                *next_id += 1;
            } else {
                if row.id >= *next_id {
                    *next_id = row.id + 1;
                }
            }
            ids.push(row.id);
            entry.insert(row.id, row);
        }
        ids
    }

    pub fn get_rows(&self, table_name: &str) -> Vec<&Row> {
        self.tables
            .get(&normalize(table_name))
            .map(|map| map.values().collect())
            .unwrap_or_default()
    }

    pub fn get_table_mut(&mut self, table_name: &str) -> Option<&mut BTreeMap<u64, Row>> {
        self.tables.get_mut(&normalize(table_name))
    }

    pub fn get_row(&self, table_name: &str, row_id: u64) -> Option<&Row> {
        self.tables
            .get(&normalize(table_name))
            .and_then(|map| map.get(&row_id))
    }

    pub fn clear_table(&mut self, table_name: &str) {
        let tn = normalize(table_name);
        // Remove all indexes for this table
        let index_names: Vec<String> = self
            .index_data
            .keys()
            .filter(|k| k.starts_with(&format!("{}:", tn)))
            .cloned()
            .collect();
        for name in index_names {
            self.index_data.remove(&name);
        }
        self.tables.remove(&tn);
        self.next_ids.remove(&tn);
    }
}
