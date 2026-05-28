# AGENTS.md

## Repository purpose
`srrdb` is a Rust MySQL-compatible database server with its own SQL execution engine, storage, WAL, persistence format, and protocol implementation.

## Essential commands
Only commands observed in this repo/docs:

```bash
# Build
cargo build --release

# Run server (default 127.0.0.1:3307)
cargo run

# Run server (release)
cargo run --release

# Built-in network client
cargo run --release -- --client

# In-process SQL parser REPL (not the DB server)
cargo run -- --repl

# CLI options
cargo run -- --help

# Test suite (integration-heavy)
cargo test
```

## Codebase map (high-value files)
- `src/main.rs`: mode switch (`--repl`, `--client`, else server).
- `src/config.rs`: merges CLI + optional TOML config (`--config`), CLI overrides file.
- `src/server/listener.rs`: startup/recovery path, builds shared `Executor`, accepts TCP connections.
- `src/server/connection.rs`: MySQL handshake/auth + command loop + SQL execution dispatch.
- `src/engine/executor.rs`: core SQL behavior (DDL/DML/query, index maintenance, save/WAL hooks).
- `src/engine/catalog.rs`: logical schema metadata, DB/table/index registry.
- `src/engine/storage.rs`: in-memory row/index storage structures.
- `src/engine/wal.rs`: WAL format, replay, checkpoint truncation.
- `src/engine/persistence.rs`: bincode file persistence + atomic file writes.
- `src/client.rs`: built-in wire client; supports `source <path>` and `schema` meta-command.
- `tests/integration.rs`: end-to-end behavior coverage via MySQL protocol.

## Runtime architecture and control flow
1. `main` loads `Config` and chooses REPL/client/server mode.
2. Server mode (`listener::start`) initializes persistence and recovery:
   - if existing data: tries checkpoint replay from WAL bytes first,
   - falls back to catalog/table file load + WAL replay,
   - then rebuilds in-memory indexes.
3. TCP accept loop spawns one Tokio task per connection.
4. Each connection performs MySQL handshake/auth then loops on commands.
5. `COM_QUERY` is parsed by `sqlparser` MySQL dialect and each statement is executed through `Executor`.

## Persistence + recovery model (important gotchas)
- Data directory defaults to `data` (relative path).
- On-disk artifacts:
  - `catalog.srrdb` (catalog metadata),
  - `tables/*.srrdb` (table rows),
  - `indexes/*.idx` (index data),
  - `srrdb.wal` (operation log/checkpoint payload).
- Mutations call `log_wal(...)` before/around state updates in executor paths.
- `DELETE`/`UPDATE` persist via `WalEntry::TableSnapshot` (full table snapshot entry), not row-delta WAL entries.
- `Persistence::atomic_write` writes `*.tmp` then renames; avoid bypassing this pattern when adding persisted files.
- Startup always calls `executor.rebuild_indexes()`; if changing index serialization/recovery, keep this flow coherent.

## Naming/case conventions that matter
- Table/database/index lookups are largely normalized to lowercase in catalog/storage/index keys.
- Index storage keys use `"{table_lower}:{index_lower}"` format in memory and persistence calls.
- Many SQL-facing checks are case-insensitive (`eq_ignore_ascii_case`).
- When adding metadata lookups, follow existing normalize/lowercase behavior or features will become case-fragile.

## SQL/engine behavior notes
- SQL parsing uses `sqlparser` MySQL dialect (`sqlparser::dialect::MySqlDialect`).
- `Executor::execute` is a large `Statement` match; extend support there and wire persistence/index side effects in the same change.
- Query optimization currently attempts equality-based index lookup (`try_index_lookup`) and falls back to scan + filter.
- `SHOW TABLES` column name is dynamic (`Tables_in_<db>`), so tests/clients should not hardcode a generic column name.
- Session default DB is `srrdb`; `USE` updates per-connection session state.

## Protocol/client/server gotchas
- Default auth is optional: if `default_password` is unset, connections are accepted without password verification.
- `connection.rs` explicitly treats unknown command `27` as OK (compatibility behavior); avoid removing unless intentionally breaking clients.
- Resultset serialization is text protocol only (`protocol/resultset.rs`), so binary-protocol features are absent.
- Built-in client (`src/client.rs`) has convenience commands not part of SQL itself:
  - `source <file>` executes semicolon-split statements,
  - `schema` runs `SHOW TABLES` + `DESCRIBE` each table.

## Testing strategy and constraints
- Primary verification is `tests/integration.rs` using `mysql_async` against a spawned TCP server.
- Integration tests share a fixed port (`3307`) and a one-time static server startup flag; parallel external use of the same port can cause failures.
- Test helper server path uses `Executor::new(...)` (no persistence, no configured password), so persistence/auth changes may need targeted additional tests.

## Working conventions for future agents
- Keep changes surgical in `executor.rs`; most SQL features require coordinated updates across:
  1) statement dispatch,
  2) data mutation,
  3) index maintenance,
  4) WAL/persistence save paths,
  5) integration tests.
- For new protocol behavior, update both server (`connection.rs`/`protocol/*`) and built-in client expectations (`client.rs`) when needed.
- Prefer extending existing helpers (`normalize`, index key builders, save/log helpers) instead of adding alternative code paths.
