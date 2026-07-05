use anyhow::Result;
use quinn::{RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::types::{ChunkInfo, ControlMessage, FileMeta};
use crate::transfer::chunk::Chunk;

/// Sends file chunks to a receiver over a QUIC bidirectional stream.
///
/// Protocol per stream:
///   1. Sender writes JSON `ControlMessage::TransferRequest` (length-prefixed)
///   2. Receiver writes JSON `ControlMessage::TransferAccept`
///   3. Sender writes each chunk: [chunk_index: u64 LE] [chunk_size: u32 LE] [data] [hash: 32 bytes]
///   4. Sender writes JSON `ControlMessage::TransferDone`
pub struct FileSender {
    send: SendStream,
    recv: RecvStream,
    file_meta: FileMeta,
    bytes_sent: u64,
}

impl FileSender {
    pub fn new(send: SendStream, recv: RecvStream, file_meta: FileMeta) -> Self {
        Self {
            send,
            recv,
            file_meta,
            bytes_sent: 0,
        }
    }

    /// Send the transfer request header and wait for accept.
    pub async fn handshake(&mut self) -> Result<ControlMessage> {
        let msg = ControlMessage::TransferRequest {
            transfer_id: crate::types::TransferId::new_v4(),
            file_meta: self.file_meta.clone(),
        };
        send_json(&mut self.send, &msg).await?;

        let resp: ControlMessage = recv_json(&mut self.recv).await?;
        Ok(resp)
    }

    /// Send one chunk over the stream.
    pub async fn send_chunk(&mut self, chunk: &Chunk) -> Result<()> {
        let header = ChunkInfo {
            index: chunk.index,
            offset: chunk.offset,
            size: chunk.data.len(),
            hash: chunk.hash,
        };
        // Write chunk header as JSON
        let header_json = serde_json::to_vec(&header)?;
        self.send
            .write_u32_le(header_json.len() as u32)
            .await?;
        self.send.write_all(&header_json).await?;
        // Write chunk data
        self.send.write_all(&chunk.data).await?;

        self.bytes_sent += chunk.data.len() as u64;
        Ok(())
    }

    /// Signal that transfer is complete.
    pub async fn finish(&mut self) -> Result<()> {
        let msg = ControlMessage::TransferDone {
            transfer_id: crate::types::TransferId::new_v4(),
        };
        send_json(&mut self.send, &msg).await?;
        self.send.finish()?;
        Ok(())
    }

    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent
    }
}

/// Write a length-prefixed JSON message to a stream.
pub(crate) async fn send_json(stream: &mut SendStream, msg: &impl serde::Serialize) -> Result<()> {
    let data = serde_json::to_vec(msg)?;
    stream.write_u32_le(data.len() as u32).await?;
    stream.write_all(&data).await?;
    Ok(())
}

/// Read a length-prefixed JSON message from a stream.
pub(crate) async fn recv_json<T: serde::de::DeserializeOwned>(
    stream: &mut RecvStream,
) -> Result<T> {
    let len = stream.read_u32_le().await?;
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}
