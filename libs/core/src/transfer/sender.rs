use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::transfer::chunk::Chunk;
use crate::transport::TcpStream;
use crate::types::{ChunkInfo, ControlMessage, FileMeta};

/// Sends file chunks to a receiver over a single TCP connection.
///
/// Protocol:
///   1. Sender writes JSON `ControlMessage::TransferRequest` (length-prefixed)
///   2. Receiver writes JSON `ControlMessage::TransferAccept`
///   3. Sender writes each chunk: [chunk_info JSON len: u32 LE] [JSON] [chunk data]
///   4. Sender writes JSON `ControlMessage::TransferDone`
pub struct FileSender {
    stream: TcpStream,
    file_meta: FileMeta,
    bytes_sent: u64,
}

impl FileSender {
    pub fn new(stream: TcpStream, file_meta: FileMeta) -> Self {
        Self {
            stream,
            file_meta,
            bytes_sent: 0,
        }
    }

    /// Send the transfer request and wait for accept.
    pub async fn handshake(&mut self) -> Result<ControlMessage> {
        let msg = ControlMessage::TransferRequest {
            transfer_id: crate::types::TransferId::new_v4(),
            file_meta: self.file_meta.clone(),
        };
        send_json(&mut self.stream, &msg).await?;
        let resp: ControlMessage = recv_json(&mut self.stream).await?;
        Ok(resp)
    }

    /// Send one chunk.
    pub async fn send_chunk(&mut self, chunk: &Chunk) -> Result<()> {
        let header = ChunkInfo {
            index: chunk.index,
            offset: chunk.offset,
            size: chunk.data.len(),
            hash: chunk.hash,
        };
        let header_json = serde_json::to_vec(&header)?;
        self.stream
            .write_u32_le(header_json.len() as u32)
            .await?;
        self.stream.write_all(&header_json).await?;
        self.stream.write_all(&chunk.data).await?;
        self.bytes_sent += chunk.data.len() as u64;
        Ok(())
    }

    /// Signal transfer complete.
    pub async fn finish(&mut self) -> Result<()> {
        let msg = ControlMessage::TransferDone {
            transfer_id: crate::types::TransferId::new_v4(),
        };
        send_json(&mut self.stream, &msg).await?;
        Ok(())
    }

    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent
    }

    pub fn into_inner(self) -> TcpStream {
        self.stream
    }
}

// ---------------------------------------------------------------------------
// Length-prefixed JSON helpers
// ---------------------------------------------------------------------------

pub(crate) async fn send_json(
    stream: &mut TcpStream,
    msg: &impl serde::Serialize,
) -> Result<()> {
    let data = serde_json::to_vec(msg)?;
    stream.write_u32_le(data.len() as u32).await?;
    stream.write_all(&data).await?;
    Ok(())
}

pub(crate) async fn recv_json<T: serde::de::DeserializeOwned>(
    stream: &mut TcpStream,
) -> Result<T> {
    let len = stream.read_u32_le().await?;
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}
