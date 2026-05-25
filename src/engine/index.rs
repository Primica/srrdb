use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::engine::types::Value;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum IndexType {
    BTree,
    Hash,
}

impl std::fmt::Display for IndexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexType::BTree => write!(f, "BTREE"),
            IndexType::Hash => write!(f, "HASH"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: String,
    pub index_type: IndexType,
    pub table_name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexData {
    BTree(BTreeMap<Vec<Value>, Vec<u64>>),
    Hash(HashMap<Vec<Value>, Vec<u64>>),
}

impl IndexData {
    pub fn new(index_type: IndexType) -> Self {
        match index_type {
            IndexType::BTree => IndexData::BTree(BTreeMap::new()),
            IndexType::Hash => IndexData::Hash(HashMap::new()),
        }
    }

    pub fn insert(&mut self, key: &[Value], row_id: u64) {
        match self {
            IndexData::BTree(map) => {
                map.entry(key.to_vec()).or_default().push(row_id);
            }
            IndexData::Hash(map) => {
                map.entry(key.to_vec()).or_default().push(row_id);
            }
        }
    }

    pub fn delete(&mut self, key: &[Value], row_id: u64) {
        match self {
            IndexData::BTree(map) => {
                if let std::collections::btree_map::Entry::Occupied(mut entry) =
                    map.entry(key.to_vec())
                {
                    entry.get_mut().retain(|id| *id != row_id);
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                }
            }
            IndexData::Hash(map) => {
                if let std::collections::hash_map::Entry::Occupied(mut entry) =
                    map.entry(key.to_vec())
                {
                    entry.get_mut().retain(|id| *id != row_id);
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                }
            }
        }
    }

    pub fn lookup_eq(&self, key: &[Value]) -> Vec<u64> {
        match self {
            IndexData::BTree(map) => map.get(key).cloned().unwrap_or_default(),
            IndexData::Hash(map) => map.get(key).cloned().unwrap_or_default(),
        }
    }

    pub fn lookup_range(
        &self,
        low: Option<Vec<Value>>,
        high: Option<Vec<Value>>,
    ) -> Vec<u64> {
        match self {
            IndexData::BTree(map) => {
                let range: Box<dyn Iterator<Item = (&Vec<Value>, &Vec<u64>)>> = match (low, high)
                {
                    (Some(l), Some(h)) => Box::new(map.range(l..=h)),
                    (Some(l), None) => Box::new(map.range(l..)),
                    (None, Some(h)) => Box::new(map.range(..=h)),
                    (None, None) => Box::new(map.iter()),
                };
                range.flat_map(|(_, ids)| ids.clone()).collect()
            }
            IndexData::Hash(_) => Vec::new(),
        }
    }

    pub fn lookup_prefix(&self, prefix: &[Value]) -> Vec<u64> {
        match self {
            IndexData::BTree(map) => {
                let start = prefix.to_vec();
                let end = {
                    let mut v = prefix.to_vec();
                    if let Some(last) = v.last_mut() {
                        *last = bump_value(last);
                    }
                    v
                };
                map.range(start..end)
                    .flat_map(|(_, ids)| ids.clone())
                    .collect()
            }
            IndexData::Hash(_) => Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        match self {
            IndexData::BTree(map) => map.clear(),
            IndexData::Hash(map) => map.clear(),
        }
    }
}

fn bump_value(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::Int(n) => Value::Int(n.saturating_add(1)),
        Value::UInt(n) => Value::UInt(n.saturating_add(1)),
        Value::Float(n) => Value::Float(n + 1.0),
        Value::Double(n) => Value::Double(n + 1.0),
        Value::Bytes(b) => {
            let mut b = b.clone();
            b.push(0);
            Value::Bytes(b)
        }
        Value::Text(s) => {
            let mut s = s.clone();
            s.push('\0');
            Value::Text(s)
        }
    }
}
