use std::sync::Arc;

use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};

use crate::engine::executor::{ExecuteResult, Executor};
use crate::protocol::command::{parse_command, Command};
use crate::protocol::frame::{read_packet, write_packet};
use crate::protocol::handshake::{
    build_handshake, eof_payload, err_payload, generate_scramble, ok_payload,
    parse_handshake_response, verify_native_password_hash, CLIENT_CONNECT_WITH_DB,
    CLIENT_LONG_PASSWORD, CLIENT_MULTI_RESULTS, CLIENT_PLUGIN_AUTH, CLIENT_PROTOCOL_41,
    CLIENT_PS_MULTI_RESULTS, CLIENT_SECURE_CONNECTION, CLIENT_TRANSACTIONS,
    SERVER_STATUS_AUTOCOMMIT,
};
use crate::protocol::resultset::{
    column_count_payload, column_definition_payload, text_row_payload,
};
use crate::protocol::Result;
use crate::server::session::{Session, User};

pub async fn handle_connection(
    mut stream: TcpStream,
    executor: Arc<Executor>,
    default_password: Option<String>,
) -> Result<()> {
    let scramble = generate_scramble();
    let caps = CLIENT_PROTOCOL_41
        | CLIENT_SECURE_CONNECTION
        | CLIENT_PLUGIN_AUTH
        | CLIENT_CONNECT_WITH_DB
        | CLIENT_MULTI_RESULTS
        | CLIENT_PS_MULTI_RESULTS
        | CLIENT_LONG_PASSWORD
        | CLIENT_TRANSACTIONS;

    write_packet(&mut stream, 0, &build_handshake(1, &scramble, caps, 45)).await?;

    let packet = read_packet(&mut stream).await?;
    let response = parse_handshake_response(&packet.payload)?;

    let user = match &default_password {
        Some(password) => {
            let user_obj = User::with_password(&response.username, password);
            let valid = match user_obj.password_hash.as_ref() {
                Some(stored_hash) => {
                    verify_native_password_hash(stored_hash, &scramble, &response.auth_response)
                }
                None => false,
            };
            if !valid {
                warn!("Authentication failed for user {}", response.username);
                let p = err_payload(1045, "Access denied for user");
                write_packet(&mut stream, 2, &p).await?;
                return Ok(());
            }
            info!("User {} authenticated successfully", response.username);
            user_obj
        }
        None => {
            info!("User {} connected (no password required)", response.username);
            User::new(&response.username)
        }
    };

    let mut session = Session::new_with_user(user);

    write_packet(&mut stream, 2, &ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT)).await?;

    info!("Session established: user={}, db={}", session.user.name, session.database);

    loop {
        let packet = match read_packet(&mut stream).await {
            Ok(p) => p,
            Err(_) => break,
        };
        let mut seq = packet.seq_id.wrapping_add(1);

        let cmd = match parse_command(&packet.payload) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to parse command: {e}");
                let p = err_payload(1064, &e.to_string());
                write_packet(&mut stream, seq, &p).await?;
                seq = seq.wrapping_add(1);
                continue;
            }
        };

        match cmd {
            Command::Quit => {
                info!("Client disconnected");
                break;
            }
            Command::Ping => {
                debug!("Ping");
                let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
                write_packet(&mut stream, seq, &p).await?;
                seq = seq.wrapping_add(1);
            }
            Command::InitDb(db) => {
                debug!("InitDb: {db}");
                if executor.database_exists(&db) {
                    session.database = db;
                    let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
                    write_packet(&mut stream, seq, &p).await?;
                } else {
                    let p = err_payload(1049, &format!("Unknown database"));
                    write_packet(&mut stream, seq, &p).await?;
                }
                seq = seq.wrapping_add(1);
            }
            Command::Query(sql) => {
                debug!("Query: {sql}");
                match handle_query(&mut stream, &mut seq, &executor, &mut session, &sql).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!("Query error: {e}");
                        let p = err_payload(1064, &e);
                        write_packet(&mut stream, seq, &p).await?;
                        seq = seq.wrapping_add(1);
                    }
                }
            }
            Command::Unknown(27, _) => {
                let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
                write_packet(&mut stream, seq, &p).await?;
                seq = seq.wrapping_add(1);
            }
            Command::Unknown(cmd_id, _) => {
                warn!("Unknown command: {cmd_id}");
                let msg = format!("Unknown command: {cmd_id}");
                let p = err_payload(1045, &msg);
                write_packet(&mut stream, seq, &p).await?;
                seq = seq.wrapping_add(1);
            }
        }
    }

    Ok(())
}

async fn handle_query(
    stream: &mut TcpStream,
    seq: &mut u8,
    executor: &Executor,
    session: &mut Session,
    sql: &str,
) -> std::result::Result<(), String> {
    let statements =
        sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::MySqlDialect {}, sql)
            .map_err(|e| format!("{e}"))?;

    if statements.is_empty() {
        let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
        write_packet(stream, *seq, &p).await.map_err(|e| format!("{e}"))?;
        *seq = seq.wrapping_add(1);
        return Ok(());
    }

    for statement in &statements {
        let result = executor.execute(&session.database, statement)?;

        match result {
            ExecuteResult::Rows { columns, rows } => {
                if columns.is_empty() {
                    let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
                    write_packet(stream, *seq, &p).await.map_err(|e| format!("{e}"))?;
                    *seq = seq.wrapping_add(1);
                } else {
                    write_packet(stream, *seq, &column_count_payload(columns.len()))
                        .await.map_err(|e| format!("{e}"))?;
                    *seq = seq.wrapping_add(1);

                    for col in &columns {
                        write_packet(stream, *seq, &column_definition_payload(col))
                            .await.map_err(|e| format!("{e}"))?;
                        *seq = seq.wrapping_add(1);
                    }

                    write_packet(stream, *seq, &eof_payload())
                        .await.map_err(|e| format!("{e}"))?;
                    *seq = seq.wrapping_add(1);

                    for row in &rows {
                        write_packet(stream, *seq, &text_row_payload(&row.values))
                            .await.map_err(|e| format!("{e}"))?;
                        *seq = seq.wrapping_add(1);
                    }

                    write_packet(stream, *seq, &eof_payload())
                        .await.map_err(|e| format!("{e}"))?;
                    *seq = seq.wrapping_add(1);
                }
            }
            ExecuteResult::Affected { rows, last_insert_id } => {
                let p = ok_payload(rows, last_insert_id, SERVER_STATUS_AUTOCOMMIT);
                write_packet(stream, *seq, &p).await.map_err(|e| format!("{e}"))?;
                *seq = seq.wrapping_add(1);
            }
            ExecuteResult::DatabaseChanged(new_db) => {
                session.database = new_db.clone();
                let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
                write_packet(stream, *seq, &p).await.map_err(|e| format!("{e}"))?;
                *seq = seq.wrapping_add(1);
            }
            ExecuteResult::Ok => {
                let p = ok_payload(0, 0, SERVER_STATUS_AUTOCOMMIT);
                write_packet(stream, *seq, &p).await.map_err(|e| format!("{e}"))?;
                *seq = seq.wrapping_add(1);
            }
        }
    }

    Ok(())
}
