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

    let has_data = persistence.has_existing_data();

    let (catalog, storage, wal) = if has_data {
        persistence.ensure_dirs()?;
        let mut wal = Wal::new(&config.data_dir);
        wal.open()?;

        // Try checkpoint recovery first
        let wal_bytes = std::fs::read(wal.path())?;
        let (cat, stg) = if !wal_bytes.is_empty() {
            match crate::engine::wal::replay_checkpoint(&wal_bytes) {
                Ok((cat, stg)) => {
                    info!("Recovered from WAL checkpoint");
                    (cat, stg)
                }
                Err(_) => {
                    info!("WAL checkpoint recovery failed, trying file-based + replay");
                    let mut catalog = persistence.load_catalog()
                        .unwrap_or_else(|| {
                            info!("No existing catalog found, starting fresh");
                            Catalog::new()
                        });

                    let mut stg = Storage::new();
                    for db in catalog.databases.values() {
                        for table_name in db.tables.keys() {
                            if let Some(rows) = persistence.load_table_data(table_name) {
                                stg.insert_rows(table_name, rows);
                                info!("Loaded table {table_name}");
                            }
                        }
                    }

                    // Replay WAL entries on top of loaded state
                    let replayed = wal.replay(&mut catalog, &mut stg)?;
                    if replayed > 0 {
                        info!("Replayed {replayed} WAL entries");
                    }

                    (catalog, stg)
                }
            }
        } else {
            // WAL exists but is empty — load from file-based storage
            let catalog = persistence.load_catalog()
                .unwrap_or_else(|| {
                    info!("No existing catalog found, starting fresh");
                    Catalog::new()
                });

            let mut stg = Storage::new();
            for db in catalog.databases.values() {
                for table_name in db.tables.keys() {
                    if let Some(rows) = persistence.load_table_data(table_name) {
                        stg.insert_rows(table_name, rows);
                        info!("Loaded table {table_name}");
                    }
                }
            }

            (catalog, stg)
        };

        (Arc::new(Mutex::new(cat)), Arc::new(Mutex::new(stg)), Arc::new(Mutex::new(wal)))
    } else {
        info!("Starting fresh — no existing data found");
        let catalog = Arc::new(Mutex::new(Catalog::new()));
        let storage = Arc::new(Mutex::new(Storage::new()));
        let wal = Wal::new(&config.data_dir);
        (catalog, storage, Arc::new(Mutex::new(wal)))
    };

    let executor = Arc::new(Executor::with_wal(
        catalog.clone(),
        storage.clone(),
        persistence,
        wal.clone(),
    ));

    executor.rebuild_indexes();

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
