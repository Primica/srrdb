use crate::engine::types::{Column, Value};
use crate::protocol::handshake::{lenenc_int_bytes, lenenc_str};

pub fn column_count_payload(count: usize) -> Vec<u8> {
    lenenc_int_bytes(count as u64)
}

pub fn column_definition_payload(column: &Column) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&lenenc_str(b"def"));
    p.extend_from_slice(&lenenc_str(b""));
    p.extend_from_slice(&lenenc_str(column.table.as_bytes()));
    p.extend_from_slice(&lenenc_str(column.table.as_bytes()));
    p.extend_from_slice(&lenenc_str(column.name.as_bytes()));
    p.extend_from_slice(&lenenc_str(column.name.as_bytes()));
    p.push(0x0C);
    p.extend_from_slice(&column.charset.to_le_bytes());
    p.extend_from_slice(&column.column_length.to_le_bytes());
    p.push(column.col_type as u8);
    p.extend_from_slice(&column.flags.to_le_bytes());
    p.push(column.decimals);
    p.extend_from_slice(&[0u8; 2]);
    p
}

pub fn text_row_payload(values: &[Value]) -> Vec<u8> {
    let mut p = Vec::new();
    for val in values {
        match val {
            Value::Null => p.push(0xFB),
            Value::Int(v) => p.extend_from_slice(&lenenc_str(v.to_string().as_bytes())),
            Value::UInt(v) => p.extend_from_slice(&lenenc_str(v.to_string().as_bytes())),
            Value::Float(v) => p.extend_from_slice(&lenenc_str(format!("{v}").as_bytes())),
            Value::Double(v) => p.extend_from_slice(&lenenc_str(format!("{v}").as_bytes())),
            Value::Bytes(b) => p.extend_from_slice(&lenenc_str(b)),
            Value::Text(s) => p.extend_from_slice(&lenenc_str(s.as_bytes())),
        }
    }
    p
}
