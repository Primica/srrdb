use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DefaultExpr {
    Value(Value),
    CurrentTimestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Float(f32),
    Double(f64),
    Bytes(Vec<u8>),
    Text(String),
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Value::Null => 0u8.hash(state),
            Value::Int(v) => {
                1u8.hash(state);
                v.hash(state);
            }
            Value::UInt(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            Value::Float(v) => {
                3u8.hash(state);
                v.to_bits().hash(state);
            }
            Value::Double(v) => {
                4u8.hash(state);
                v.to_bits().hash(state);
            }
            Value::Bytes(v) => {
                5u8.hash(state);
                v.hash(state);
            }
            Value::Text(v) => {
                6u8.hash(state);
                v.hash(state);
            }
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let variant_order = |v: &Value| -> u8 {
            match v {
                Value::Null => 0,
                Value::Int(_) => 1,
                Value::UInt(_) => 2,
                Value::Float(_) => 3,
                Value::Double(_) => 4,
                Value::Bytes(_) => 5,
                Value::Text(_) => 6,
            }
        };
        match (self, other) {
            (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::UInt(a), Value::UInt(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
            (Value::Double(a), Value::Double(b)) => a.total_cmp(b),
            (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            _ => variant_order(self).cmp(&variant_order(other)),
        }
    }
}

impl Value {
    pub fn to_string(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Int(v) => v.to_string(),
            Value::UInt(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Double(v) => v.to_string(),
            Value::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            Value::Text(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ColumnType {
    Decimal = 0,
    Tiny = 1,
    Short = 2,
    Long = 3,
    Float = 4,
    Double = 5,
    Null = 6,
    Timestamp = 7,
    LongLong = 8,
    Int24 = 9,
    Date = 10,
    Time = 11,
    DateTime = 12,
    Year = 13,
    VarChar = 15,
    Bit = 16,
    Json = 245,
    NewDecimal = 246,
    Enum = 247,
    Set = 248,
    TinyBlob = 249,
    MediumBlob = 250,
    LongBlob = 251,
    Blob = 252,
    VarString = 253,
    String = 254,
    Geometry = 255,
}

impl ColumnType {
    pub fn from_sql_type(sql_type: &str) -> Self {
        match sql_type.to_uppercase().as_str() {
            "INT" | "INTEGER" | "INT4" => ColumnType::Long,
            "BIGINT" | "INT8" => ColumnType::LongLong,
            "SMALLINT" | "INT2" => ColumnType::Short,
            "TINYINT" => ColumnType::Tiny,
            "FLOAT" => ColumnType::Float,
            "DOUBLE" | "REAL" => ColumnType::Double,
            "DECIMAL" | "NUMERIC" => ColumnType::NewDecimal,
            "VARCHAR" | "CHARACTER VARYING" => ColumnType::VarChar,
            "CHAR" => ColumnType::String,
            "TEXT" | "CLOB" => ColumnType::Blob,
            "BOOLEAN" | "BOOL" => ColumnType::Tiny,
            "BLOB" => ColumnType::Blob,
            "DATE" => ColumnType::Date,
            "TIMESTAMP" => ColumnType::Timestamp,
            "JSON" => ColumnType::Json,
            _ => ColumnType::VarChar,
        }
    }

    pub fn default_length(&self) -> u32 {
        match self {
            ColumnType::Tiny => 4,
            ColumnType::Short => 6,
            ColumnType::Long => 11,
            ColumnType::LongLong => 20,
            ColumnType::Float => 12,
            ColumnType::Double => 22,
            ColumnType::VarChar | ColumnType::String => 255,
            ColumnType::Blob | ColumnType::VarString => 65535,
            ColumnType::Date => 10,
            ColumnType::Timestamp => 19,
            _ => 255,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub table: String,
    pub col_type: ColumnType,
    pub column_length: u32,
    pub charset: u16,
    pub flags: u16,
    pub decimals: u8,
    #[serde(default)]
    pub auto_increment: bool,
    #[serde(default)]
    pub default_expr: Option<DefaultExpr>,
}

impl Column {
    pub fn new(name: &str, table: &str, col_type: ColumnType) -> Self {
        let len = col_type.default_length();
        Column {
            name: name.to_string(),
            table: table.to_string(),
            col_type,
            column_length: len,
            charset: 45, // utf8mb4_general_ci
            flags: 0,
            decimals: 0,
            auto_increment: false,
            default_expr: None,
        }
    }
}
