use tokio::net::TcpStream;

use crate::protocol::frame::{read_packet, write_packet};
use crate::protocol::handshake::{
    CLIENT_CONNECT_WITH_DB, CLIENT_PLUGIN_AUTH, CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA,
    CLIENT_PROTOCOL_41, CLIENT_SECURE_CONNECTION, CLIENT_TRANSACTIONS, COM_QUERY,
};

struct ServerHandshake {
    _protocol_version: u8,
    _server_version: String,
    _connection_id: u32,
    scramble: [u8; 20],
    capabilities: u32,
    _charset: u8,
    _auth_plugin: Option<String>,
}

fn parse_server_handshake(payload: &[u8]) -> Result<ServerHandshake, String> {
    let mut pos = 0;
    let protocol_version = payload[pos];
    pos += 1;

    let end = payload[pos..]
        .iter()
        .position(|&b| b == 0)
        .ok_or("missing null terminator for server version")?;
    let server_version =
        String::from_utf8(payload[pos..pos + end].to_vec()).map_err(|e| e.to_string())?;
    pos += end + 1;

    let connection_id = u32::from_le_bytes(payload[pos..pos + 4].try_into().unwrap());
    pos += 4;

    let mut scramble = [0u8; 20];
    scramble[..8].copy_from_slice(&payload[pos..pos + 8]);
    pos += 8;

    pos += 1;

    let capabilities_lower = u16::from_le_bytes(payload[pos..pos + 2].try_into().unwrap());
    pos += 2;

    let charset = payload[pos];
    pos += 1;

    pos += 2;

    let capabilities_upper = u16::from_le_bytes(payload[pos..pos + 2].try_into().unwrap());
    pos += 2;
    let capabilities = (capabilities_upper as u32) << 16 | capabilities_lower as u32;

    let auth_plugin_data_len = if capabilities & CLIENT_PLUGIN_AUTH != 0 {
        payload[pos]
    } else {
        0
    };
    pos += 1;

    pos += 10;

    if capabilities & CLIENT_SECURE_CONNECTION != 0 {
        let part2_len = (auth_plugin_data_len as usize).saturating_sub(9);
        if part2_len > 0 && pos + part2_len <= payload.len() {
            let copy_len = part2_len.min(12);
            scramble[8..8 + copy_len].copy_from_slice(&payload[pos..pos + copy_len]);
        }
    }

    let auth_plugin = if capabilities & CLIENT_PLUGIN_AUTH != 0 {
        if pos < payload.len() {
            let end = payload[pos..]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(payload.len() - pos);
            String::from_utf8(payload[pos..pos + end].to_vec()).ok()
        } else {
            None
        }
    } else {
        None
    };

    Ok(ServerHandshake {
        _protocol_version: protocol_version,
        _server_version: server_version,
        _connection_id: connection_id,
        scramble,
        capabilities,
        _charset: charset,
        _auth_plugin: auth_plugin,
    })
}

fn compute_auth_response(password: &[u8], scramble: &[u8]) -> Vec<u8> {
    use sha1::{Digest, Sha1};

    let password_hash = {
        let mut hasher = Sha1::new();
        hasher.update(password);
        hasher.finalize()
    };

    let double_hash = {
        let mut hasher = Sha1::new();
        hasher.update(&password_hash);
        hasher.finalize()
    };

    let mut hasher = Sha1::new();
    hasher.update(scramble);
    hasher.update(&double_hash);
    let hash = hasher.finalize();

    password_hash
        .iter()
        .zip(hash.iter())
        .map(|(a, b)| a ^ b)
        .collect()
}

fn build_handshake_response(handshake: &ServerHandshake, username: &str, password: &str) -> Vec<u8> {
    let mut caps = CLIENT_PROTOCOL_41
        | CLIENT_SECURE_CONNECTION
        | CLIENT_PLUGIN_AUTH
        | CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA
        | CLIENT_TRANSACTIONS
        | CLIENT_CONNECT_WITH_DB;

    caps &= handshake.capabilities;

    let mut p = Vec::new();
    p.extend_from_slice(&caps.to_le_bytes());
    p.extend_from_slice(&16777215u32.to_le_bytes());
    p.push(255);
    p.extend_from_slice(&[0u8; 23]);

    p.extend_from_slice(username.as_bytes());
    p.push(0);

    let auth = if password.is_empty() {
        Vec::new()
    } else {
        compute_auth_response(password.as_bytes(), &handshake.scramble)
    };

    p.push(auth.len() as u8);
    p.extend_from_slice(&auth);

    p.extend_from_slice(b"srrdb");
    p.push(0);

    p.extend_from_slice(b"mysql_native_password");
    p.push(0);

    p
}

fn parse_lenenc_int(data: &[u8]) -> (u64, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    match data[0] {
        0xFB => (0, 1),
        0xFC => (u16::from_le_bytes([data[1], data[2]]) as u64, 3),
        0xFD => {
            (data[1] as u64 | (data[2] as u64) << 8 | (data[3] as u64) << 16, 4)
        }
        0xFE => (u64::from_le_bytes(data[1..9].try_into().unwrap()), 9),
        _ => (data[0] as u64, 1),
    }
}

fn parse_lenenc_str(data: &[u8]) -> (&[u8], usize) {
    let (len, n) = parse_lenenc_int(data);
    let len = len as usize;
    (&data[n..n + len], n + len)
}

#[derive(Debug)]
struct ColumnDef {
    name: Vec<u8>,
}

fn parse_column_def(data: &[u8]) -> Result<ColumnDef, String> {
    let mut pos = 0;
    let (_, n) = parse_lenenc_str(&data[pos..]);
    pos += n;
    let (_, n) = parse_lenenc_str(&data[pos..]);
    pos += n;
    let (_, n) = parse_lenenc_str(&data[pos..]);
    pos += n;
    let (_, n) = parse_lenenc_str(&data[pos..]);
    pos += n;
    let (v, _) = parse_lenenc_str(&data[pos..]);
    let name = v.to_vec();

    Ok(ColumnDef { name })
}

enum QueryResult {
    Ok {
        affected_rows: u64,
        last_insert_id: u64,
    },
    Error {
        code: u16,
        message: String,
    },
    ResultSet {
        columns: Vec<ColumnDef>,
        rows: Vec<Vec<Option<Vec<u8>>>>,
    },
}

async fn execute_query(stream: &mut TcpStream, sql: &str) -> Result<QueryResult, String> {
    let mut payload = vec![COM_QUERY];
    payload.extend_from_slice(sql.as_bytes());
    write_packet(stream, 0, &payload)
        .await
        .map_err(|e| e.to_string())?;

    let pkt = read_packet(stream).await.map_err(|e| e.to_string())?;

    if pkt.payload.is_empty() {
        return Err("empty response".to_string());
    }

    let first = pkt.payload[0];
    if first == 0x00 {
        let mut pos = 1;
        let (affected_rows, n) = parse_lenenc_int(&pkt.payload[pos..]);
        pos += n;
        let (last_insert_id, _) = parse_lenenc_int(&pkt.payload[pos..]);
        Ok(QueryResult::Ok {
            affected_rows,
            last_insert_id,
        })
    } else if first == 0xFF {
        let code = u16::from_le_bytes(pkt.payload[1..3].try_into().unwrap());
        let msg = String::from_utf8_lossy(&pkt.payload[3..]).to_string();
        Ok(QueryResult::Error {
            code,
            message: msg,
        })
    } else if first == 0xFE {
        Ok(QueryResult::Ok {
            affected_rows: 0,
            last_insert_id: 0,
        })
    } else {
        let (column_count, _) = parse_lenenc_int(&pkt.payload);

        let mut columns = Vec::new();
        for _ in 0..column_count {
            let pkt = read_packet(stream).await.map_err(|e| e.to_string())?;
            let col = parse_column_def(&pkt.payload)?;
            columns.push(col);
        }

        let _eof = read_packet(stream).await.map_err(|e| e.to_string())?;

        let mut rows = Vec::new();
        loop {
            let pkt = read_packet(stream).await.map_err(|e| e.to_string())?;
            if pkt.payload.is_empty() || pkt.payload[0] == 0xFE {
                break;
            }
            let mut row = Vec::new();
            let mut pos = 0;
            while pos < pkt.payload.len() {
                if pkt.payload[pos] == 0xFB {
                    row.push(None);
                    pos += 1;
                } else {
                    let (val, n) = parse_lenenc_str(&pkt.payload[pos..]);
                    row.push(Some(val.to_vec()));
                    pos += n;
                }
            }
            rows.push(row);
        }

        Ok(QueryResult::ResultSet { columns, rows })
    }
}

fn format_result(result: &QueryResult) {
    match result {
        QueryResult::Ok {
            affected_rows,
            last_insert_id,
        } => {
            println!(
                "Query OK, {} row(s) affected (last insert id: {})",
                affected_rows, last_insert_id
            );
        }
        QueryResult::Error { code, message } => {
            println!("ERROR {}: {}", code, message);
        }
        QueryResult::ResultSet { columns, rows } => {
            let col_names: Vec<&str> = columns
                .iter()
                .map(|c| std::str::from_utf8(&c.name).unwrap_or("?"))
                .collect();

            let mut col_widths: Vec<usize> = col_names.iter().map(|n| n.len()).collect();
            for row in rows {
                for (i, val) in row.iter().enumerate() {
                    if i < col_widths.len() {
                        let len = val.as_ref().map(|v| v.len()).unwrap_or(4);
                        col_widths[i] = col_widths[i].max(len);
                    }
                }
            }

            print_border(&col_widths);
            print_row(&col_names, &col_widths, '|');
            print_border(&col_widths);

            for row in rows {
                let display: Vec<&str> = row
                    .iter()
                    .map(|v| match v {
                        Some(d) => std::str::from_utf8(d).unwrap_or("?"),
                        None => "NULL",
                    })
                    .collect();
                print_row(&display, &col_widths, '|');
            }

            print_border(&col_widths);

            let plural = if rows.len() == 1 { "" } else { "s" };
            println!("{} row{} in set", rows.len(), plural);
        }
    }
}

fn print_border(widths: &[usize]) {
    print!("+");
    for w in widths {
        print!("-{:-<w$}-+", "");
    }
    println!();
}

fn print_row(values: &[&str], widths: &[usize], sep: char) {
    print!("{sep}");
    for (i, val) in values.iter().enumerate() {
        if i < widths.len() {
            print!(" {:<width$} {sep}", val, width = widths[i]);
        }
    }
    println!();
}

pub async fn run_client(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> Result<(), String> {
    let addr = format!("{}:{}", host, port);
    println!("Connecting to {} as {}", addr, user);

    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("connection failed: {e}"))?;

    let pkt = read_packet(&mut stream)
        .await
        .map_err(|e| format!("read handshake failed: {e}"))?;

    if pkt.payload[0] == 0xFF {
        let code = u16::from_le_bytes(pkt.payload[1..3].try_into().unwrap());
        let msg = String::from_utf8_lossy(&pkt.payload[3..]).to_string();
        return Err(format!("server error {code}: {msg}"));
    }

    let handshake = parse_server_handshake(&pkt.payload)?;

    let response = build_handshake_response(&handshake, user, password);
    write_packet(&mut stream, 1, &response)
        .await
        .map_err(|e| format!("send auth failed: {e}"))?;

    let auth_result = read_packet(&mut stream)
        .await
        .map_err(|e| format!("read auth result failed: {e}"))?;

    if auth_result.payload.is_empty() {
        return Err("empty auth response".to_string());
    }

    if auth_result.payload[0] == 0xFF {
        let code = u16::from_le_bytes(auth_result.payload[1..3].try_into().unwrap());
        let msg = String::from_utf8_lossy(&auth_result.payload[3..]).to_string();
        return Err(format!("auth error {code}: {msg}"));
    }

    println!("Connected.");
    println!("Type 'exit' or 'quit' to disconnect.\n");

    let mut multi_line_buf = String::new();
    loop {
        let prompt = if multi_line_buf.is_empty() {
            format!("srrdb ({})> ", user)
        } else {
            "    -> ".to_string()
        };

        let line = read_line(&prompt);
        match line {
            Ok(line) => {
                let trimmed = line.trim().to_string();

                if trimmed.eq_ignore_ascii_case("exit")
                    || trimmed.eq_ignore_ascii_case("quit")
                    || trimmed.eq_ignore_ascii_case("\\q")
                {
                    break;
                }

                if let Some(path) = trimmed.strip_prefix("source ") {
                    if !multi_line_buf.is_empty() {
                        multi_line_buf.clear();
                    }
                    let path = path.trim().trim_end_matches(';').trim_matches('"').trim_matches('\'');
                    match std::fs::read_to_string(path) {
                        Ok(content) => {
                            for statement in content.split(';') {
                                let stmt = statement.trim();
                                if !stmt.is_empty() {
                                    match execute_query(&mut stream, stmt).await {
                                        Ok(result) => format_result(&result),
                                        Err(e) => println!("Error: {}", e),
                                    }
                                }
                            }
                        }
                        Err(e) => println!("Error reading file '{}': {}", path, e),
                    }
                    continue;
                }

                if trimmed.is_empty() && !multi_line_buf.is_empty() {
                    continue;
                }

                if trimmed.is_empty() {
                    continue;
                }

                multi_line_buf.push_str(&trimmed);
                multi_line_buf.push(' ');

                if trimmed.ends_with(';') {
                    let sql = multi_line_buf.trim().to_string();
                    multi_line_buf.clear();

                    match execute_query(&mut stream, &sql).await {
                        Ok(result) => format_result(&result),
                        Err(e) => println!("Error: {}", e),
                    }
                }
            }
            Err(_) => {
                break;
            }
        }
    }

    println!("Bye.");
    Ok(())
}

fn read_line(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if line.is_empty() {
        return Err("EOF".into());
    }
    Ok(line.trim_end_matches('\n').to_string())
}
