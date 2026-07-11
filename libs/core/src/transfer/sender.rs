use anyhow::Result;
use tokio::io::AsyncWriteExt;

use crate::transfer::chunk::Chunk;
use crate::transport::TcpStream;
use crate::types::{ControlMessage, FileMeta};

/// Binary chunk header on wire (56 bytes total).
///
/// Layout (little-endian):
///   [0..4]   type:  u32    (1 = chunk, 0xFFFFFFFF = done)
///   [4..12]  index: u64
///   [12..20] offset: u64
///   [20..24] size:  u32    (bytes of following chunk data)
///   [24..56] hash:  [u8; 32]
const HDR_SIZE: usize = 56;
const CHUNK_TYPE: u32 = 1;
const DONE_TYPE: u32 = 0xFFFFFFFF;

fn pack_header(ty: u32, index: u64, offset: u64, size: u32, hash: &[u8; 32]) -> [u8; HDR_SIZE] {
    let mut buf = [0u8; HDR_SIZE];
    buf[0..4].copy_from_slice(&ty.to_le_bytes());
    buf[4..12].copy_from_slice(&index.to_le_bytes());
    buf[12..20].copy_from_slice(&offset.to_le_bytes());
    buf[20..24].copy_from_slice(&size.to_le_bytes());
    buf[24..56].copy_from_slice(hash);
    buf
}

/// Sends file chunks to a receiver over a single TCP connection.
///
/// Protocol:
///   1. JSON handshake: TransferRequest / TransferAccept
///   2. Binary chunks: [56-byte header] [data]
///   3. Binary done:   [56-byte header with type=0xFFFFFFFF]
pub struct FileSender {
    stream: TcpStream,
    file_meta: FileMeta,
    bytes_sent: u64,
}

impl FileSender {
    pub fn new(stream: TcpStream, file_meta: FileMeta) -> Self {
        Self { stream, file_meta, bytes_sent: 0 }
    }

    /// Send the transfer request and wait for accept (JSON).
    pub async fn handshake(&mut self) -> Result<ControlMessage> {
        let msg = ControlMessage::TransferRequest {
            transfer_id: crate::types::TransferId::new_v4(),
            file_meta: self.file_meta.clone(),
        };
        send_json(&mut self.stream, &msg).await?;
        let resp: ControlMessage = recv_json(&mut self.stream).await?;
        Ok(resp)
    }

    /// Send one chunk with a binary header.
    pub async fn send_chunk(&mut self, chunk: &Chunk) -> Result<()> {
        let hdr = pack_header(
            CHUNK_TYPE,
            chunk.index,
            chunk.offset,
            chunk.data.len() as u32,
            &chunk.hash,
        );
        self.stream.write_all(&hdr).await?;
        self.stream.write_all(&chunk.data).await?;
        self.bytes_sent += chunk.data.len() as u64;
        Ok(())
    }

    /// Signal transfer complete (binary done marker) and shut down the write side
    /// so the receiver sees a proper EOF instead of a TCP RST.
    pub async fn finish(&mut self) -> Result<()> {
        let hdr = pack_header(DONE_TYPE, 0, 0, 0, &[0u8; 32]);
        self.stream.write_all(&hdr).await?;
        self.stream.shutdown().await?;
        Ok(())
    }

    pub fn bytes_sent(&self) -> u64 { self.bytes_sent }
    pub fn into_inner(self) -> TcpStream { self.stream }
}

// ---------------------------------------------------------------------------
// Length-prefixed JSON helpers (used for handshake only)
// ---------------------------------------------------------------------------

pub async fn send_json(stream: &mut TcpStream, msg: &impl serde::Serialize) -> Result<()> {
    let data = serde_json::to_vec(msg)?;
    stream.write_u32_le(data.len() as u32).await?;
    stream.write_all(&data).await?;
    Ok(())
}

pub async fn recv_json<T: serde::de::DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    use tokio::io::AsyncReadExt;
    let len = stream.read_u32_le().await?;
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}
