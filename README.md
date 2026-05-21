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
- Built-in MySQL wire protocol client (`--client` flag)
- Interactive REPL for in-process testing
- Concurrent connections via Tokio async
- Full SQL support: CREATE/DROP TABLE, CREATE/DROP DATABASE, INSERT, SELECT, DELETE, UPDATE, USE, SHOW, DESCRIBE/DESC
- WHERE with comparison operators, AND/OR, LIKE, BETWEEN, IN, IS NULL
- ORDER BY (ASC/DESC), LIMIT, OFFSET
- Database management: CREATE DATABASE, DROP DATABASE, USE, SHOW DATABASES, SHOW TABLES
- DESCRIBE / DESC — view table column info (Field, Type, Null, Key, Default, Extra)
- Column features: AUTO_INCREMENT, DEFAULT (literals + NOW()/CURRENT_TIMESTAMP)
- Client-side `source <path>` command to execute SQL files
- Interactive prompt with database indicator: `srrdb (root@db_name)> `

## Requirements

- **Rust** 1.75+ (edition 2024)
- **Cargo** (included with Rust)
- No external dependencies — the binary includes both server and client

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
# Terminal 1: Start the server (default: 127.0.0.1:3307)
cargo run --release

# Terminal 2: Connect with the built-in client (no MySQL needed)
cargo run --release -- --client

# Or use the interactive REPL (in-process, no network)
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
  -H, --host <HOST>                  Server bind address or client connect host
  -P, --port <PORT>                  Server bind port or client connect port
      --data-dir <DATA_DIR>          Data directory for persistence
      --log-level <LOG_LEVEL>        Log level (trace, debug, info, warn, error)
      --default-password <PASSWORD>  Default password for all users
      --repl                         Start interactive REPL (in-process)
      --client                       Start interactive client (network)
  -u, --user <USER>                  Client username (default: root)
  -p, --password <PASSWORD>          Client password
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

### Connect with the built-in client

```bash
# Default: connect to 127.0.0.1:3307 as root
cargo run --release -- --client

# Custom connection
cargo run --release -- --client -H 192.168.1.10 -P 3307 -u admin -p secret

# Or using a MySQL client (if you have one installed)
mysql -h 127.0.0.1 -P 3307 -u root srrdb
```

The client prompt shows the connected database after any `USE` command:
```
srrdb (root)>            ← no database selected yet
srrdb (root@blog)>       ← after USE blog
srrdb (root@testdb)>     ← after USE testdb
```

The client also supports `source <path>` to execute SQL from a file:
```bash
echo "source /path/to/script.sql;" | cargo run --release -- --client
```

The client supports a `schema` meta-command that shows the complete schema
of all tables in the current database (runs `SHOW TABLES` then `DESCRIBE` on each):
```
srrdb (root@blog)> schema

── USERS ──
+---------------+---------+------+-----+---------+----------------+
| Field         | Type    | Null | Key | Default | Extra          |
+---------------+---------+------+-----+---------+----------------+
| USER_ID       | int     | YES  | PRI | NULL    | auto_increment |
| USERNAME      | varchar | YES  |     | NULL    |                |
| ...           | ...     | ...  | ... | ...     | ...            |
+---------------+---------+------+-----+---------+----------------+
6 rows in set
```

## Supported SQL

### Database Management

- `CREATE DATABASE <name>`
- `DROP DATABASE <name>` / `DROP DATABASE IF EXISTS <name>`
- `USE <name>` / `USE DATABASE <name>`
- `SHOW DATABASES`
- `SHOW TABLES`

### Data Definition

- `CREATE TABLE` / `CREATE TABLE IF NOT EXISTS` — INT, BIGINT, SMALLINT, TINYINT, FLOAT, DOUBLE,
  VARCHAR, CHAR, TEXT, BOOLEAN, DATE, DATETIME, TIMESTAMP, JSON, BLOB, DECIMAL
- `DROP TABLE` / `DROP TABLE IF EXISTS`
- Column options: `AUTO_INCREMENT`, `PRIMARY KEY`, `DEFAULT <expr>`, `FOREIGN KEY` (accepted,
  constraints not enforced)
- `DESCRIBE <table>` / `DESC <table>` — show column metadata (Field, Type, Null, Key, Default, Extra)

### Data Manipulation

- `INSERT INTO ... VALUES ( ... )` — multiple rows, with optional column list
- `INSERT INTO t (col1, col2) VALUES (val1, val2)` — mapped to correct columns, missing
  columns get their DEFAULT value, AUTO_INCREMENT, or NULL
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
  main.rs     Binary entry point (server, client, or REPL)
  sql/        SQL lexer and parser (wraps sqlparser-rs)
  engine/     Catalog, storage, persistence, WAL, query executor
  protocol/   MySQL wire protocol (framing, handshake, auth, commands, results)
  server/     TCP listener and per-connection handler
  client.rs   Built-in MySQL wire protocol client (no external dependencies)
  repl/       Interactive REPL for in-process testing
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
