use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};
use tracing::warn;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 3307;
const DEFAULT_DATA_DIR: &str = "data";
const DEFAULT_LOG_LEVEL: &str = "info";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub data_dir: Option<String>,
    pub log_level: Option<String>,
    pub default_password: Option<String>,
}

#[derive(Debug, Clone, Parser)]
#[command(name = "srrdb", version, about = "MySQL-compatible database server")]
pub struct CliArgs {
    #[arg(short, long)]
    pub config: Option<String>,

    #[arg(short = 'H', long)]
    pub host: Option<String>,

    #[arg(short = 'P', long)]
    pub port: Option<u16>,

    #[arg(long)]
    pub data_dir: Option<String>,

    #[arg(long)]
    pub log_level: Option<String>,

    #[arg(long)]
    pub default_password: Option<String>,

    #[arg(long)]
    pub repl: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub log_level: String,
    pub default_password: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        let args = CliArgs::parse();

        let mut config = Config {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            default_password: None,
        };

        if let Some(config_path) = &args.config {
            if let Ok(content) = std::fs::read_to_string(config_path) {
                if let Ok(file_config) = toml::from_str::<ConfigFile>(&content) {
                    if let Some(host) = file_config.host {
                        config.host = host;
                    }
                    if let Some(port) = file_config.port {
                        config.port = port;
                    }
                    if let Some(data_dir) = file_config.data_dir {
                        config.data_dir = PathBuf::from(data_dir);
                    }
                    if let Some(log_level) = file_config.log_level {
                        config.log_level = log_level;
                    }
                    if let Some(password) = file_config.default_password {
                        config.default_password = Some(password);
                    }
                }
            } else {
                warn!("Could not read config file: {config_path}");
            }
        }

        if let Some(host) = args.host {
            config.host = host;
        }
        if let Some(port) = args.port {
            config.port = port;
        }
        if let Some(data_dir) = args.data_dir {
            config.data_dir = PathBuf::from(data_dir);
        }
        if let Some(log_level) = args.log_level {
            config.log_level = log_level;
        }
        if let Some(password) = args.default_password {
            config.default_password = Some(password);
        }

        config
    }

    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
