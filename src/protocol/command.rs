use crate::protocol::handshake::{COM_INIT_DB, COM_PING, COM_QUERY, COM_QUIT};
use crate::protocol::Result;

#[derive(Debug)]
pub enum Command {
    Quit,
    InitDb(String),
    Query(String),
    Ping,
    Unknown(u8, Vec<u8>),
}

pub fn parse_command(payload: &[u8]) -> Result<Command> {
    if payload.is_empty() {
        return Err("empty command packet".into());
    }
    let cmd_id = payload[0];
    let args = payload[1..].to_vec();

    match cmd_id {
        COM_QUIT => Ok(Command::Quit),
        COM_INIT_DB => {
            let db = String::from_utf8(args)?;
            Ok(Command::InitDb(db))
        }
        COM_QUERY => {
            let sql = String::from_utf8(args)?;
            Ok(Command::Query(sql))
        }
        COM_PING => Ok(Command::Ping),
        _ => Ok(Command::Unknown(cmd_id, args)),
    }
}
