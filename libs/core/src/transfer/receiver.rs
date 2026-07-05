use anyhow::Result;
use tokio::io::AsyncReadExt;

use crate::transport::TcpStream;
use crate::types::{ChunkInfo, ControlMessage, FileMeta};
use crate::transfer::sender::{recv_json, send_json};

/// Receives file chunks from a sender over a single TCP connection.
pub struct FileReceiver {
    stream: TcpStream,
    pub file_meta: Option<FileMeta>,
    bytes_received: u64,
}

impl FileReceiver {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            file_meta: None,
            bytes_received: 0,
        }
    }

    /// Read the TransferRequest and send back an accept.
    pub async fn handshake(&mut self) -> Result<ControlMessage> {
        let msg: ControlMessage = recv_json(&mut self.stream).await?;
        match msg {
            ControlMessage::TransferRequest {
                transfer_id: _,
                file_meta,
            } => {
                self.file_meta = Some(file_meta);
                let accept = ControlMessage::TransferAccept {
                    transfer_id: crate::types::TransferId::new_v4(),
                    received_chunks: vec![],
                };
                send_json(&mut self.stream, &accept).await?;
                Ok(accept)
            }
            _ => anyhow::bail!("expected TransferRequest, got {:?}", msg),
        }
    }

    /// Read the next chunk.
    /// Returns `None` when the transfer is done or stream ends.
    pub async fn recv_chunk(&mut self) -> Result<Option<(ChunkInfo, Vec<u8>)>> {
        let mut len_buf = [0u8; 4];
        match self.stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => anyhow::bail!(e),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;

        if let Ok(info) = serde_json::from_slice::<ChunkInfo>(&buf) {
            let mut data = vec![0u8; info.size];
            self.stream.read_exact(&mut data).await?;
            self.bytes_received += data.len() as u64;
            Ok(Some((info, data)))
        } else if let Ok(ControlMessage::TransferDone { .. }) =
            serde_json::from_slice(&buf)
        {
            Ok(None)
        } else {
            anyhow::bail!("unexpected message on stream");
        }
    }

    pub fn bytes_received(&self) -> u64 {
        self.bytes_received
    }

    pub fn into_inner(self) -> TcpStream {
        self.stream
    }
}
