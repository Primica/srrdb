use sha1::{Digest, Sha1};

use crate::protocol::Result;

const SCRAMBLE_LEN: usize = 20;

pub fn generate_scramble() -> [u8; SCRAMBLE_LEN] {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut buf = [0u8; SCRAMBLE_LEN];
    rng.fill(&mut buf);
    buf
}

pub fn build_handshake(
    conn_id: u32,
    scramble: &[u8; SCRAMBLE_LEN],
    capabilities: u32,
    charset: u8,
) -> Vec<u8> {
    let mut p = Vec::new();

    p.push(10);
    p.extend_from_slice(b"srrdb 0.2.0\0");
    p.extend_from_slice(&conn_id.to_le_bytes());
    p.extend_from_slice(&scramble[..8]);
    p.push(0x00);
    p.extend_from_slice(&(capabilities as u16).to_le_bytes());
    p.push(charset);
    p.extend_from_slice(&2u16.to_le_bytes());
    p.extend_from_slice(&((capabilities >> 16) as u16).to_le_bytes());

    if capabilities & CLIENT_PLUGIN_AUTH != 0 {
        p.push(SCRAMBLE_LEN as u8 + 1);
    } else {
        p.push(0);
    }

    p.extend_from_slice(&[0u8; 10]);

    if capabilities & CLIENT_SECURE_CONNECTION != 0 {
        p.extend_from_slice(&scramble[8..]);
        p.push(0x00);
    }

    if capabilities & CLIENT_PLUGIN_AUTH != 0 {
        p.extend_from_slice(b"mysql_native_password\0");
    }

    p
}

#[derive(Debug)]
pub struct HandshakeResponse {
    pub capabilities: u32,
    pub max_packet_size: u32,
    pub charset: u8,
    pub username: String,
    pub auth_response: Vec<u8>,
    pub database: Option<String>,
    pub auth_plugin: Option<String>,
}

pub fn parse_handshake_response(payload: &[u8]) -> Result<HandshakeResponse> {
    let mut pos = 0;

    let capabilities = u32::from_le_bytes(payload[pos..pos + 4].try_into()?);
    pos += 4;
    let max_packet_size = u32::from_le_bytes(payload[pos..pos + 4].try_into()?);
    pos += 4;
    let charset = payload[pos];
    pos += 1;
    pos += 23;

    let end = payload[pos..]
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| format!("expected null-terminated username"))?;
    let username = String::from_utf8(payload[pos..pos + end].to_vec())?;
    pos += end + 1;

    let auth_response = if capabilities & CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA != 0 {
        let (len, n) = lenenc_int(&payload[pos..]);
        pos += n;
        let data = payload[pos..pos + len].to_vec();
        pos += len;
        data
    } else if capabilities & CLIENT_SECURE_CONNECTION != 0 {
        let len = payload[pos] as usize;
        pos += 1;
        let data = payload[pos..pos + len].to_vec();
        pos += len;
        data
    } else {
        let end = payload[pos..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| format!("expected null-terminated auth response"))?;
        let data = payload[pos..pos + end].to_vec();
        pos += end + 1;
        data
    };

    let database = if capabilities & CLIENT_CONNECT_WITH_DB != 0 {
        let end = payload[pos..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| format!("expected null-terminated database"))?;
        let db = String::from_utf8(payload[pos..pos + end].to_vec())?;
        pos += end + 1;
        Some(db)
    } else {
        None
    };

    let auth_plugin = if capabilities & CLIENT_PLUGIN_AUTH != 0 {
        let end = payload[pos..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| format!("expected null-terminated auth plugin"))?;
        let plugin = String::from_utf8(payload[pos..pos + end].to_vec())?;
        Some(plugin)
    } else {
        None
    };

    Ok(HandshakeResponse {
        capabilities,
        max_packet_size,
        charset,
        username,
        auth_response,
        database,
        auth_plugin,
    })
}

pub fn verify_native_password(
    password: &str,
    scramble: &[u8; SCRAMBLE_LEN],
    auth_response: &[u8],
) -> bool {
    let password_hash = {
        let mut hasher = Sha1::new();
        hasher.update(password.as_bytes());
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

    let expected: Vec<u8> = password_hash
        .iter()
        .zip(hash.iter())
        .map(|(a, b)| a ^ b)
        .collect();

    auth_response == expected.as_slice()
}

/// Verify a mysql_native_password auth response using a stored SHA1(SHA1(password)) hash.
/// Returns true if the client's auth_response matches the expected value.
pub fn verify_native_password_hash(
    stored_double_hash: &[u8; 20],
    scramble: &[u8; SCRAMBLE_LEN],
    auth_response: &[u8],
) -> bool {
    let mut hasher = Sha1::new();
    hasher.update(scramble);
    hasher.update(stored_double_hash);
    let hash = hasher.finalize();

    let expected_password_hash: Vec<u8> = auth_response
        .iter()
        .zip(hash.iter())
        .map(|(a, b)| a ^ b)
        .collect();

    let mut hasher = Sha1::new();
    hasher.update(&expected_password_hash);
    let result = hasher.finalize();

    result.as_slice() == stored_double_hash
}

/// Compute SHA1(SHA1(password)) for storage.
pub fn hash_native_password(password: &str) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(password.as_bytes());
    let hash1 = hasher.finalize();

    let mut hasher = Sha1::new();
    hasher.update(&hash1);
    let result = hasher.finalize();

    let mut out = [0u8; 20];
    out.copy_from_slice(&result);
    out
}

pub fn ok_payload(affected_rows: u64, last_insert_id: u64, status: u16) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(0x00);
    p.extend_from_slice(&lenenc_int_bytes(affected_rows));
    p.extend_from_slice(&lenenc_int_bytes(last_insert_id));
    p.extend_from_slice(&status.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    p
}

pub fn err_payload(code: u16, message: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(0xFF);
    p.extend_from_slice(&code.to_le_bytes());
    p.push(b'#');
    p.extend_from_slice(b"HY000");
    p.extend_from_slice(message.as_bytes());
    p
}

pub fn eof_payload() -> Vec<u8> {
    let mut p = Vec::new();
    p.push(0xFE);
    p.extend_from_slice(&2u16.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    p
}

pub fn lenenc_int_bytes(v: u64) -> Vec<u8> {
    if v < 251 {
        vec![v as u8]
    } else if v < 65536 {
        let mut b = vec![0xFC];
        b.extend_from_slice(&(v as u16).to_le_bytes());
        b
    } else if v < 16777216 {
        let mut b = vec![0xFD];
        b.extend_from_slice(&(v as u32).to_le_bytes()[..3]);
        b
    } else {
        let mut b = vec![0xFE];
        b.extend_from_slice(&v.to_le_bytes());
        b
    }
}

fn lenenc_int(data: &[u8]) -> (usize, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    match data[0] {
        0xFC => (u16::from_le_bytes([data[1], data[2]]) as usize, 3),
        0xFD => {
            let v = data[1] as u32 | (data[2] as u32) << 8 | (data[3] as u32) << 16;
            (v as usize, 4)
        }
        0xFE => (u64::from_le_bytes(data[1..9].try_into().unwrap()) as usize, 9),
        _ => (data[0] as usize, 1),
    }
}

pub fn lenenc_str(data: &[u8]) -> Vec<u8> {
    let mut b = lenenc_int_bytes(data.len() as u64);
    b.extend_from_slice(data);
    b
}

#[allow(dead_code)]
pub const CLIENT_LONG_PASSWORD: u32 = 1;
pub const CLIENT_FOUND_ROWS: u32 = 2;
#[allow(dead_code)]
pub const CLIENT_LONG_FLAG: u32 = 4;
pub const CLIENT_CONNECT_WITH_DB: u32 = 8;
#[allow(dead_code)]
pub const CLIENT_COMPRESS: u32 = 32;
#[allow(dead_code)]
pub const CLIENT_LOCAL_FILES: u32 = 128;
#[allow(dead_code)]
pub const CLIENT_IGNORE_SPACE: u32 = 256;
pub const CLIENT_PROTOCOL_41: u32 = 512;
#[allow(dead_code)]
pub const CLIENT_INTERACTIVE: u32 = 1024;
#[allow(dead_code)]
pub const CLIENT_SSL: u32 = 2048;
pub const CLIENT_TRANSACTIONS: u32 = 8192;
pub const CLIENT_SECURE_CONNECTION: u32 = 32768;
#[allow(dead_code)]
pub const CLIENT_MULTI_STATEMENTS: u32 = 65536;
pub const CLIENT_MULTI_RESULTS: u32 = 131072;
pub const CLIENT_PS_MULTI_RESULTS: u32 = 262144;
pub const CLIENT_PLUGIN_AUTH: u32 = 524288;
#[allow(dead_code)]
pub const CLIENT_CONNECT_ATTRS: u32 = 1048576;
pub const CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA: u32 = 2097152;
#[allow(dead_code)]
pub const CLIENT_CAN_HANDLE_EXPIRED_PASSWORDS: u32 = 4194304;
#[allow(dead_code)]
pub const CLIENT_SESSION_TRACK: u32 = 8388608;
#[allow(dead_code)]
pub const CLIENT_DEPRECATE_EOF: u32 = 16777216;

pub const SERVER_STATUS_AUTOCOMMIT: u16 = 2;

pub const COM_SLEEP: u8 = 0;
pub const COM_QUIT: u8 = 1;
pub const COM_INIT_DB: u8 = 2;
pub const COM_QUERY: u8 = 3;
pub const COM_PING: u8 = 14;
