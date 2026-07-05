use anyhow::Result;
use tokio::io::AsyncReadExt;

use crate::transport::TcpStream;
use crate::types::{ChunkInfo, ControlMessage, FileMeta};
use crate::transfer::sender::{recv_json, send_json};

/// Binary chunk header on wire (56 bytes).
const HDR_SIZE: usize = 56;
const CHUNK_TYPE: u32 = 1;
const DONE_TYPE: u32 = 0xFFFFFFFF;

fn parse_header(buf: &[u8; HDR_SIZE]) -> (u32, u64, u64, u32, [u8; 32]) {
    let ty = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let index = u64::from_le_bytes(buf[4..12].try_into().unwrap());
    let offset = u64::from_le_bytes(buf[12..20].try_into().unwrap());
    let size = u32::from_le_bytes(buf[20..24].try_into().unwrap());
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&buf[24..56]);
    (ty, index, offset, size, hash)
}

/// Receives file chunks from a sender over a single TCP connection.
pub struct FileReceiver {
    stream: TcpStream,
    pub file_meta: Option<FileMeta>,
    bytes_received: u64,
}

impl FileReceiver {
    pub fn new(stream: TcpStream) -> Self {
        Self { stream, file_meta: None, bytes_received: 0 }
    }

    /// Read the TransferRequest and send back an accept (JSON).
    pub async fn handshake(&mut self) -> Result<ControlMessage> {
        let msg: ControlMessage = recv_json(&mut self.stream).await?;
        match msg {
            ControlMessage::TransferRequest { transfer_id: _, file_meta } => {
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

    /// Read the next chunk (binary header + data).
    /// Returns `None` on done marker.
    pub async fn recv_chunk(&mut self) -> Result<Option<(ChunkInfo, Vec<u8>)>> {
        let mut hdr = [0u8; HDR_SIZE];
        match self.stream.read_exact(&mut hdr).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => anyhow::bail!(e),
        }

        let (ty, index, offset, size, hash) = parse_header(&hdr);
        if ty == DONE_TYPE {
            return Ok(None);
        }
        if ty != CHUNK_TYPE {
            anyhow::bail!("unknown chunk type: {}", ty);
        }

        let mut data = vec![0u8; size as usize];
        self.stream.read_exact(&mut data).await?;
        self.bytes_received += size as u64;

        let info = ChunkInfo { index, offset, size: size as usize, hash };
        Ok(Some((info, data)))
    }

    pub fn bytes_received(&self) -> u64 { self.bytes_received }
    pub fn into_inner(self) -> TcpStream { self.stream }
}
