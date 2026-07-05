use anyhow::Result;
use quinn::{RecvStream, SendStream};

use crate::types::{ChunkInfo, ControlMessage, FileMeta};
use crate::transfer::sender::{recv_json, send_json};

/// Receives file chunks from a sender over a QUIC bidirectional stream.
pub struct FileReceiver {
    recv: RecvStream,
    send: SendStream,
    pub file_meta: Option<FileMeta>,
    bytes_received: u64,
}

impl FileReceiver {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self {
            send,
            recv,
            file_meta: None,
            bytes_received: 0,
        }
    }

    /// Read the initial TransferRequest and send back an accept.
    pub async fn handshake(&mut self) -> Result<ControlMessage> {
        let msg: ControlMessage = recv_json(&mut self.recv).await?;
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
                send_json(&mut self.send, &accept).await?;
                Ok(accept)
            }
            _ => anyhow::bail!("expected TransferRequest, got {:?}", msg),
        }
    }

    /// Read the next chunk from the stream.
    /// Returns `None` when the transfer is done.
    pub async fn recv_chunk(&mut self) -> Result<Option<(ChunkInfo, Vec<u8>)>> {
        let mut len_buf = [0u8; 4];
        match self.recv.read_exact(&mut len_buf).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(_)) => return Ok(None),
            Err(e) => anyhow::bail!(e),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.recv.read_exact(&mut buf).await?;

        if let Ok(info) = serde_json::from_slice::<ChunkInfo>(&buf) {
            let mut data = vec![0u8; info.size];
            self.recv.read_exact(&mut data).await?;
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
}
