use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::engine::catalog::Catalog;
use crate::engine::storage::{Row, Storage};
use crate::engine::types::Column;

const MAGIC: [u8; 2] = [0x53, 0x44]; // "SD" = srrdb
const ENTRY_HEADER_SIZE: usize = 7; // magic(2) + entry_len(4) + entry_type(1)

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WalEntryType {
    CreateTable = 1,
    DropTable = 2,
    InsertRows = 3,
    TableSnapshot = 4,
    Checkpoint = 255,
}

#[derive(Debug, Clone)]
pub enum WalEntry {
    CreateTable {
        table_name: String,
        columns: Vec<Column>,
    },
    DropTable {
        table_name: String,
    },
    InsertRows {
        table_name: String,
        rows: Vec<Row>,
    },
    TableSnapshot {
        table_name: String,
        rows: Vec<Row>,
    },
    Checkpoint,
}

pub struct Wal {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Wal {
    pub fn new(data_dir: &Path) -> Self {
        Wal {
            path: data_dir.join("srrdb.wal"),
            file: None,
        }
    }

    pub fn open(&mut self) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.path)?;
        self.file = Some(file);
        Ok(())
    }

    pub fn append(&mut self, entry: &WalEntry) -> std::io::Result<()> {
        let bytes = entry.serialize();
        if let Some(ref mut file) = self.file {
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        Ok(())
    }

    pub fn append_sync(&mut self, entry: &WalEntry, catalog: &Catalog, storage: &Storage) -> std::io::Result<()> {
        self.append(entry)?;
        self.truncate(catalog, storage)
    }

    pub fn replay(&self, catalog: &mut Catalog, storage: &mut Storage) -> std::io::Result<u64> {
        let mut file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };

        let mut count = 0u64;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let mut pos = 0;

        while pos + ENTRY_HEADER_SIZE <= buf.len() {
            if buf[pos..pos + 2] != MAGIC {
                warn!("WAL: invalid magic at offset {pos}, stopping replay");
                break;
            }
            let entry_len = u32::from_le_bytes([
                buf[pos + 2],
                buf[pos + 3],
                buf[pos + 4],
                buf[pos + 5],
            ]) as usize;
            let entry_type = buf[pos + 6];

            let end = pos + entry_len;
            if end > buf.len() {
                warn!("WAL: truncated entry at offset {pos}, stopping replay");
                break;
            }

            let payload = &buf[pos + ENTRY_HEADER_SIZE..end];
            match WalEntry::deserialize(entry_type, payload) {
                Ok(Some(entry)) => {
                    if apply_entry(catalog, storage, &entry) {
                        count += 1;
                    } else {
                        warn!("WAL: failed to apply entry at offset {pos}");
                    }
                }
                Ok(None) => {
                    // Checkpoint entry, no action needed
                    count += 1;
                }
                Err(e) => {
                    warn!("WAL: failed to deserialize entry at offset {pos}: {e}");
                    break;
                }
            }
            pos = end;
        }

        if count > 0 {
            info!("WAL: replayed {count} entries");
        }
        Ok(count)
    }

    pub fn truncate(&mut self, catalog: &Catalog, storage: &Storage) -> std::io::Result<()> {
        self.file = None;

        let bytes = build_checkpoint_data(catalog, storage);
        std::fs::write(&self.path, &bytes)?;

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.path)?;
        self.file = Some(file);
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl WalEntry {
    fn serialize(&self) -> Vec<u8> {
        let payload = match self {
            WalEntry::CreateTable { table_name, columns } => {
                let mut p = Vec::new();
                p.extend_from_slice(&bincode::serialize(table_name).unwrap());
                p.extend_from_slice(&bincode::serialize(columns).unwrap());
                p
            }
            WalEntry::DropTable { table_name } => {
                bincode::serialize(table_name).unwrap()
            }
            WalEntry::InsertRows { table_name, rows } => {
                let mut p = Vec::new();
                p.extend_from_slice(&bincode::serialize(table_name).unwrap());
                p.extend_from_slice(&bincode::serialize(rows).unwrap());
                p
            }
            WalEntry::TableSnapshot { table_name, rows } => {
                let mut p = Vec::new();
                p.extend_from_slice(&bincode::serialize(table_name).unwrap());
                p.extend_from_slice(&bincode::serialize(rows).unwrap());
                p
            }
            WalEntry::Checkpoint => Vec::new(),
        };

        let entry_len = (ENTRY_HEADER_SIZE + payload.len()) as u32;
        let entry_type: u8 = match self {
            WalEntry::CreateTable { .. } => WalEntryType::CreateTable as u8,
            WalEntry::DropTable { .. } => WalEntryType::DropTable as u8,
            WalEntry::InsertRows { .. } => WalEntryType::InsertRows as u8,
            WalEntry::TableSnapshot { .. } => WalEntryType::TableSnapshot as u8,
            WalEntry::Checkpoint => WalEntryType::Checkpoint as u8,
        };

        let mut buf = Vec::with_capacity(entry_len as usize);
        buf.extend_from_slice(&MAGIC);
        buf.extend_from_slice(&entry_len.to_le_bytes());
        buf.push(entry_type);
        buf.extend_from_slice(&payload);
        buf
    }

    fn deserialize(entry_type: u8, payload: &[u8]) -> std::io::Result<Option<WalEntry>> {
        match entry_type {
            1 => {
                let (table_name, offset) = bincode::deserialize::<String>(payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                    .map(|s| {
                        let s_len = bincode::serialized_size(&s).unwrap() as usize;
                        (s, s_len)
                    })?;
                let columns: Vec<Column> = bincode::deserialize(&payload[offset..])
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(WalEntry::CreateTable { table_name, columns }))
            }
            2 => {
                let table_name = bincode::deserialize(payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(WalEntry::DropTable { table_name }))
            }
            3 => {
                let (table_name, offset) = bincode::deserialize::<String>(payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                    .map(|s| {
                        let s_len = bincode::serialized_size(&s).unwrap() as usize;
                        (s, s_len)
                    })?;
                let rows: Vec<Row> = bincode::deserialize(&payload[offset..])
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(WalEntry::InsertRows { table_name, rows }))
            }
            4 => {
                let (table_name, offset) = bincode::deserialize::<String>(payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                    .map(|s| {
                        let s_len = bincode::serialized_size(&s).unwrap() as usize;
                        (s, s_len)
                    })?;
                let rows: Vec<Row> = bincode::deserialize(&payload[offset..])
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(WalEntry::TableSnapshot { table_name, rows }))
            }
            255 => Ok(Some(WalEntry::Checkpoint)),
            _ => {
                warn!("WAL: unknown entry type {entry_type}, skipping");
                Ok(None)
            }
        }
    }
}

fn apply_entry(catalog: &mut Catalog, storage: &mut Storage, entry: &WalEntry) -> bool {
    match entry {
        WalEntry::CreateTable { table_name, columns } => {
            catalog.create_table("srrdb", table_name, columns.clone()).map_err(|e| {
                warn!("WAL replay: CREATE TABLE {table_name} failed: {e}");
            }).is_ok()
        }
        WalEntry::DropTable { table_name } => {
            catalog.drop_table("srrdb", table_name).map_err(|e| {
                warn!("WAL replay: DROP TABLE {table_name} failed: {e}");
            }).is_ok() && {
                storage.clear_table(table_name);
                true
            }
        }
        WalEntry::InsertRows { table_name, rows } => {
            storage.insert_rows(table_name, rows.clone());
            true
        }
        WalEntry::TableSnapshot { table_name, rows } => {
            storage.clear_table(table_name);
            if !rows.is_empty() {
                storage.insert_rows(table_name, rows.clone());
            }
            true
        }
        WalEntry::Checkpoint => true,
    }
}

fn build_checkpoint_data(catalog: &Catalog, storage: &Storage) -> Vec<u8> {
    let mut bytes = Vec::new();

    // Write a full CHECKPOINT entry with all catalog + storage data
    let entry_type = WalEntryType::Checkpoint as u8;
    let mut payload = Vec::new();
    payload.extend_from_slice(&bincode::serialize(catalog).unwrap());

    let table_list: Vec<(&String, &Vec<Row>)> = storage.tables.iter().collect();
    payload.extend_from_slice(&bincode::serialize(&table_list.len()).unwrap());
    for (name, rows) in &storage.tables {
        payload.extend_from_slice(&bincode::serialize(name).unwrap());
        payload.extend_from_slice(&bincode::serialize(rows).unwrap());
    }

    let entry_len = (ENTRY_HEADER_SIZE + payload.len()) as u32;
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&entry_len.to_le_bytes());
    bytes.push(entry_type);
    bytes.extend_from_slice(&payload);
    bytes
}

pub fn replay_checkpoint(data: &[u8]) -> std::io::Result<(Catalog, Storage)> {
    if data.len() < ENTRY_HEADER_SIZE || data[..2] != MAGIC {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid checkpoint"));
    }
    let entry_len = u32::from_le_bytes([data[2], data[3], data[4], data[5]]) as usize;
    let entry_type = data[6];

    if entry_type != WalEntryType::Checkpoint as u8 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Not a checkpoint"));
    }

    let payload = &data[ENTRY_HEADER_SIZE..entry_len.min(data.len())];

    let catalog: Catalog = bincode::deserialize(payload)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut pos = bincode::serialized_size(&catalog).unwrap() as usize;
    if pos >= payload.len() {
        return Ok((catalog, Storage::new()));
    }

    let table_count: usize = bincode::deserialize(&payload[pos..])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    pos += bincode::serialized_size(&table_count).unwrap() as usize;

    let mut storage = Storage::new();
    for _ in 0..table_count {
        if pos >= payload.len() {
            break;
        }
        let table_name: String = bincode::deserialize(&payload[pos..])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        pos += bincode::serialized_size(&table_name).unwrap() as usize;

        if pos >= payload.len() {
            break;
        }
        let rows: Vec<Row> = bincode::deserialize(&payload[pos..])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        pos += bincode::serialized_size(&rows).unwrap() as usize;

        storage.insert_rows(&table_name, rows);
    }

    Ok((catalog, storage))
}
