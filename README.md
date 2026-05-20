# srrdb

A MySQL-compatible relational database server written in Rust from scratch —
indépendant, avec son propre moteur de stockage persistant.

## Overview

srrdb speaks the MySQL wire protocol and can be used with standard MySQL clients
(`mysql` CLI, `mysql_async`, etc.). It has its own storage engine, configuration,
authentication, and on-disk persistence — no MySQL dependency.

## Features

- MySQL wire protocol server (configurable host/port)
- `mysql_native_password` authentication (with optional password enforcement)
- SQL parsing via `sqlparser-rs` with MySQL dialect
- Persistent on-disk storage engine with Write-Ahead Log (WAL) for crash recovery
- Atomic checkpoint-based persistence (bincode serialization)
- Config file (TOML) + CLI arguments via clap
- Structured logging via `tracing`
- Text protocol queries (COM_QUERY)
- Interactive REPL for testing
- Concurrent connections via Tokio async
- Full SQL support: CREATE TABLE, INSERT, SELECT, DELETE, UPDATE, DROP TABLE
- WHERE with comparison operators, AND/OR, LIKE, BETWEEN, IN, IS NULL
- ORDER BY (ASC/DESC), LIMIT, OFFSET

## Requirements

- **Rust** 1.75+ (edition 2024)
- **Cargo** (included with Rust)
- A MySQL client for testing (`mysql` CLI, `mysql_async`, etc.)

## Installation

### Clone and build

```bash
git clone <repo-url> srrdb
cd srrdb
cargo build --release
```

The binary will be at `target/release/srrdb`.

### Quick start

```bash
# Start the server (default: 127.0.0.1:3307)
cargo run --release

# In another terminal, connect with MySQL client
mysql -h 127.0.0.1 -P 3307 -u root srrdb

# Or use the interactive REPL
cargo run --release -- --repl
```

## Usage

### Start the server

```bash
cargo run
```

The server listens on 127.0.0.1:3307 by default. The data directory `./data/`
is created automatically on first startup.

Options:
```
cargo run -- --help
srrdb 0.2.0
MySQL-compatible database server

Usage: srrdb [OPTIONS]

Options:
  -c, --config <CONFIG>              Config file path (TOML)
  -H, --host <HOST>                  Bind address
  -P, --port <PORT>                  Bind port
      --data-dir <DATA_DIR>          Data directory for persistence
      --log-level <LOG_LEVEL>        Log level (trace, debug, info, warn, error)
      --default-password <PASSWORD>  Default password for all users
      --repl                         Start interactive REPL
  -h, --help                         Print help
  -V, --version                      Print version
```

### Config file (srrdb.toml)

Create a `srrdb.toml` in the working directory:

```toml
host = "0.0.0.0"
port = 3307
data_dir = "/var/lib/srrdb"
log_level = "debug"
default_password = "secret"
```

Load it with `--config srrdb.toml`. CLI arguments override file values.

### Start the REPL

```bash
cargo run -- --repl
```

The REPL supports multi-line input (semicolon-terminated), command history, and meta-commands:
- `.help` — Show available commands
- `.tokens <sql>` — Show tokenized output
- `.ast <sql>` — Show parsed AST

### Connect with a MySQL client

```bash
mysql -h 127.0.0.1 -P 3307 -u root srrdb
```

If a `default_password` is set, use `-p`:
```bash
mysql -h 127.0.0.1 -P 3307 -u root -p srrdb
```

## Supported SQL

### Data Definition

- `CREATE TABLE` — INT, BIGINT, SMALLINT, TINYINT, FLOAT, DOUBLE, VARCHAR, CHAR, TEXT,
  BOOLEAN, DATE, TIMESTAMP, JSON, BLOB, DECIMAL
- `DROP TABLE` / `DROP TABLE IF EXISTS`

### Data Manipulation

- `INSERT INTO ... VALUES ( ... )` — multiple rows
- `DELETE FROM <table> WHERE <condition>` — delete without WHERE truncates all rows
- `UPDATE <table> SET <col> = <expr> WHERE <condition>`

### Queries

- `SELECT ... FROM ... WHERE ... ORDER BY ... LIMIT ... OFFSET ...`
- **Operators:** `=`, `!=`, `>`, `>=`, `<`, `<=`, `AND`, `OR`
- **Pattern matching:** `LIKE` — case-insensitive, wildcards `%` and `_`
- **Range:** `BETWEEN ... AND ...`
- **Set membership:** `IN (...)` — literal list
- **Null tests:** `IS NULL`, `IS NOT NULL`

## Persistence & ACID

Data is stored on disk in the configured `data_dir` (default: `./data/`):
- `srrdb.wal` — Write-Ahead Log for crash recovery
- `catalog.srrdb` — table schemas (bincode checkpoint)
- `tables/*.srrdb` — table data, one file per table (bincode checkpoint)

**Write-Ahead Log:** Every mutation (CREATE, INSERT, DELETE, UPDATE, DROP) is
appended to the WAL before being applied. On restart, the WAL is replayed to
recover any operations not yet checkpointed.

**Crash recovery:** On startup, srrdb detects the WAL, replays pending entries,
then takes a fresh checkpoint (saves full state + truncates WAL).

**Checkpoints:** After replay, a full checkpoint is written ensuring consistent state.

## Architecture

```
src/
  config.rs   Configuration file + CLI argument parsing
  lib.rs      Library root (re-exports all modules)
  main.rs     Binary entry point (server or REPL)
  sql/        SQL lexer and parser (wraps sqlparser-rs)
  engine/     Catalog, storage, persistence, WAL, query executor
  protocol/   MySQL wire protocol (framing, handshake, auth, commands, results)
  server/     TCP listener and per-connection handler
  repl/       Interactive REPL for testing
  tests/      Integration tests
```

## Tests

```bash
cargo test
```

Integration tests use `mysql_async` and cover all SQL features.

## License

MIT

Copyright (c) 2026 Primica
