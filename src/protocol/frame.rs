use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::protocol::Result;

pub struct Packet {
    pub seq_id: u8,
    pub payload: Bytes,
}

pub async fn read_packet(stream: &mut TcpStream) -> Result<Packet> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    let seq_id = header[3];
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).await?;
    }
    Ok(Packet {
        seq_id,
        payload: Bytes::from(payload),
    })
}

pub async fn write_packet(stream: &mut TcpStream, seq_id: u8, payload: &[u8]) -> Result<()> {
    let len = payload.len() as u32;
    let header = [
        (len & 0xFF) as u8,
        ((len >> 8) & 0xFF) as u8,
        ((len >> 16) & 0xFF) as u8,
        seq_id,
    ];
    stream.write_all(&header).await?;
    if !payload.is_empty() {
        stream.write_all(payload).await?;
    }
    Ok(())
}

pub fn write_payload(payload: &mut Vec<u8>, seq_id: u8, data: &[u8]) {
    let len = data.len() as u32;
    payload.extend_from_slice(&[
        (len & 0xFF) as u8,
        ((len >> 8) & 0xFF) as u8,
        ((len >> 16) & 0xFF) as u8,
        seq_id,
    ]);
    payload.extend_from_slice(data);
}
