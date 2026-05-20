use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::engine::catalog::Catalog;
use crate::engine::storage::Row;

const CATALOG_FILE: &str = "catalog.srrdb";
const DATA_DIR: &str = "tables";

pub struct Persistence {
    data_dir: PathBuf,
}

impl Persistence {
    pub fn new(data_dir: PathBuf) -> Self {
        Persistence { data_dir }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.data_dir.join(DATA_DIR))
    }

    pub fn load_catalog(&self) -> Option<Catalog> {
        let path = self.data_dir.join(CATALOG_FILE);
        if !path.exists() {
            return None;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match bincode::deserialize::<Catalog>(&bytes) {
                Ok(catalog) => {
                    info!("Loaded catalog from {}", path.display());
                    Some(catalog)
                }
                Err(e) => {
                    warn!("Failed to deserialize catalog: {e}");
                    None
                }
            },
            Err(e) => {
                warn!("Failed to read catalog file: {e}");
                None
            }
        }
    }

    pub fn save_catalog(&self, catalog: &Catalog) -> std::io::Result<()> {
        self.ensure_dirs()?;
        let path = self.data_dir.join(CATALOG_FILE);
        let bytes = bincode::serialize(catalog)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        atomic_write(&path, &bytes)?;
        info!("Saved catalog to {}", path.display());
        Ok(())
    }

    pub fn load_table_data(&self, table_name: &str) -> Option<Vec<Row>> {
        let path = self.table_path(table_name);
        if !path.exists() {
            return None;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match bincode::deserialize::<Vec<Row>>(&bytes) {
                Ok(rows) => {
                    info!("Loaded {} rows from {}", rows.len(), path.display());
                    Some(rows)
                }
                Err(e) => {
                    warn!("Failed to deserialize table data for {table_name}: {e}");
                    None
                }
            },
            Err(e) => {
                warn!("Failed to read table data for {table_name}: {e}");
                None
            }
        }
    }

    pub fn save_table_data(&self, table_name: &str, rows: &[Row]) -> std::io::Result<()> {
        self.ensure_dirs()?;
        let path = self.table_path(table_name);
        let bytes = bincode::serialize(rows)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        atomic_write(&path, &bytes)?;
        Ok(())
    }

    pub fn remove_table_data(&self, table_name: &str) -> std::io::Result<()> {
        let path = self.table_path(table_name);
        if path.exists() {
            std::fs::remove_file(&path)?;
            info!("Removed table data file {}", path.display());
        }
        Ok(())
    }

    pub fn has_existing_data(&self) -> bool {
        self.data_dir.join(CATALOG_FILE).exists()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn table_path(&self, table_name: &str) -> PathBuf {
        self.data_dir.join(DATA_DIR).join(format!("{table_name}.srrdb"))
    }
}

fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
