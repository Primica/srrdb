use std::sync::Arc;
use std::sync::Mutex;

use tokio::net::TcpListener;
use tracing::{error, info};

use crate::config::Config;
use crate::engine::catalog::Catalog;
use crate::engine::executor::Executor;
use crate::engine::persistence::Persistence;
use crate::engine::storage::Storage;
use crate::engine::wal::Wal;
use crate::server::connection::handle_connection;

pub async fn start(config: Config) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let persistence = Persistence::new(config.data_dir.clone());
    persistence.init()?;

    let mut wal = Wal::new(&config.data_dir);
    wal.open()?;

    let (catalog, storage) = if wal.path().exists() && wal.path().metadata()?.len() > 0 {
        // Try checkpoint recovery first
        let wal_bytes = std::fs::read(wal.path())?;
        match crate::engine::wal::replay_checkpoint(&wal_bytes) {
            Ok((cat, stg)) => {
                info!("Recovered from WAL checkpoint");
                (Arc::new(Mutex::new(cat)), Arc::new(Mutex::new(stg)))
            }
            Err(_) => {
                // Fallback: load from separate files, then replay WAL
                info!("WAL checkpoint recovery failed, trying file-based + replay");
                let catalog = match persistence.load_catalog() {
                    Some(cat) => {
                        info!("Loaded catalog from disk");
                        Arc::new(Mutex::new(cat))
                    }
                    None => {
                        info!("No existing catalog found, starting fresh");
                        Arc::new(Mutex::new(Catalog::new()))
                    }
                };

                let storage = {
                    let cat = catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
                    let mut stg = Storage::new();
                    for db in cat.databases.values() {
                        for table_name in db.tables.keys() {
                            if let Some(rows) = persistence.load_table_data(table_name) {
                                stg.insert_rows(table_name, rows);
                                info!("Loaded table {table_name}");
                            }
                        }
                    }
                    drop(cat);
                    Arc::new(Mutex::new(stg))
                };

                // Replay WAL entries on top of loaded state
                {
                    let mut cat = catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
                    let mut stg = storage.lock().map_err(|e| format!("Lock error: {e}"))?;
                    let replayed = wal.replay(&mut cat, &mut stg)?;
                    if replayed > 0 {
                        info!("Replayed {replayed} WAL entries");
                    }
                }

                (catalog, storage)
            }
        }
    } else {
        let catalog = match persistence.load_catalog() {
            Some(cat) => {
                info!("Loaded catalog from disk");
                Arc::new(Mutex::new(cat))
            }
            None => {
                info!("No existing catalog found, starting fresh");
                Arc::new(Mutex::new(Catalog::new()))
            }
        };

        let storage = {
            let cat = catalog.lock().map_err(|e| format!("Lock error: {e}"))?;
            let mut stg = Storage::new();
            for db in cat.databases.values() {
                for table_name in db.tables.keys() {
                    if let Some(rows) = persistence.load_table_data(table_name) {
                        stg.insert_rows(table_name, rows);
                        info!("Loaded table {table_name}");
                    }
                }
            }
            drop(cat);
            Arc::new(Mutex::new(stg))
        };

        (catalog, storage)
    };

    let wal_arc = Arc::new(Mutex::new(wal));

    let executor = Arc::new(Executor::with_wal(
        catalog.clone(),
        storage.clone(),
        persistence,
        wal_arc.clone(),
    ));

    let addr = config.addr();
    let listener = TcpListener::bind(&addr).await?;
    info!("srrdb listening on {addr}");

    let default_password = config.default_password.clone();

    loop {
        let (stream, peer) = listener.accept().await?;
        info!("New connection from {peer}");

        let executor = executor.clone();
        let password = default_password.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, executor, password).await {
                error!("Connection error ({peer}): {e}");
            }
        });
    }
}
