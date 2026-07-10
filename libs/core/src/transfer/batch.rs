use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::transfer::chunk::ChunkReader;
use crate::transfer::sender::{recv_json, send_json};
use crate::transport::TcpStream;

const CHUNK_TYPE: u32 = 1;
const DONE_TYPE: u32 = 0xFFFFFFFF;
const HDR_SIZE: usize = 56;

// ---------------------------------------------------------------------------
// Wire protocol types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchMeta {
    pub total_files: u32,
    pub total_size: u64,
    pub root_name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    pub relative_path: String,
    pub size: u64,
    #[serde(default)]
    pub compressed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ack {
    pub ok: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchDone;

// ---------------------------------------------------------------------------
// Binary header helpers
// ---------------------------------------------------------------------------

fn pack_header(ty: u32, index: u64, offset: u64, size: u32, hash: &[u8; 32]) -> [u8; HDR_SIZE] {
    let mut buf = [0u8; HDR_SIZE];
    buf[0..4].copy_from_slice(&ty.to_le_bytes());
    buf[4..12].copy_from_slice(&index.to_le_bytes());
    buf[12..20].copy_from_slice(&offset.to_le_bytes());
    buf[20..24].copy_from_slice(&size.to_le_bytes());
    buf[24..56].copy_from_slice(hash);
    buf
}

fn parse_header(buf: &[u8; HDR_SIZE]) -> (u32, u64, u64, u32, [u8; 32]) {
    let ty = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let index = u64::from_le_bytes(buf[4..12].try_into().unwrap());
    let offset = u64::from_le_bytes(buf[12..20].try_into().unwrap());
    let size = u32::from_le_bytes(buf[20..24].try_into().unwrap());
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&buf[24..56]);
    (ty, index, offset, size, hash)
}

// ---------------------------------------------------------------------------
// BatchSender
// ---------------------------------------------------------------------------

pub struct BatchSender {
    stream: TcpStream,
    meta: BatchMeta,
    chunk_size: usize,
    compress: bool,
    files_sent: u32,
    bytes_sent: u64,
}

impl BatchSender {
    pub fn new(stream: TcpStream, meta: BatchMeta, chunk_size: usize) -> Self {
        Self { stream, meta, chunk_size, compress: false, files_sent: 0, bytes_sent: 0 }
    }

    pub fn with_compression(mut self, enabled: bool) -> Self {
        self.compress = enabled;
        self
    }

    pub async fn handshake(&mut self) -> Result<()> {
        send_json(&mut self.stream, &self.meta).await?;
        let _ack: Ack = recv_json(&mut self.stream).await?;
        Ok(())
    }

    pub async fn send_file(&mut self, relative_path: &str, data: &[u8]) -> Result<()> {
        // Optionally compress
        let send_data = if self.compress {
            crate::compress::compress(data)
        } else {
            data.to_vec()
        };
        let compressed = send_data.len() < data.len();

        // File entry
        let entry = FileEntry {
            relative_path: relative_path.to_string(),
            size: data.len() as u64,
            compressed,
        };
        send_json(&mut self.stream, &entry).await?;
        let _ack: Ack = recv_json(&mut self.stream).await?;

        // Binary chunks (of possibly-compressed data)
        let reader = ChunkReader::new(&send_data[..], self.chunk_size);
        for chunk in reader {
            let chunk = chunk?;
            let hdr = pack_header(CHUNK_TYPE, chunk.index, chunk.offset, chunk.data.len() as u32, &chunk.hash);
            self.stream.write_all(&hdr).await?;
            self.stream.write_all(&chunk.data).await?;
            self.bytes_sent += chunk.data.len() as u64;
        }

        // Done marker
        let hdr = pack_header(DONE_TYPE, 0, 0, 0, &[0u8; 32]);
        self.stream.write_all(&hdr).await?;
        self.files_sent += 1;
        Ok(())
    }

    pub async fn finish(&mut self) -> Result<()> {
        let done = BatchDone;
        send_json(&mut self.stream, &done).await?;
        Ok(())
    }

    pub fn stats(&self) -> (u32, u64) {
        (self.files_sent, self.bytes_sent)
    }
}

// ---------------------------------------------------------------------------
// BatchReceiver
// ---------------------------------------------------------------------------

pub struct BatchReceiver {
    pub stream: TcpStream,
    pub meta: Option<BatchMeta>,
    pub bytes_received: u64,
}

impl BatchReceiver {
    pub fn new(stream: TcpStream) -> Self {
        Self { stream, meta: None, bytes_received: 0 }
    }

    pub async fn handshake(&mut self) -> Result<()> {
        let meta: BatchMeta = recv_json(&mut self.stream).await?;
        self.meta = Some(meta);
        let ack = Ack { ok: true };
        send_json(&mut self.stream, &ack).await?;
        Ok(())
    }

    pub async fn recv_file(&mut self) -> Result<Option<(String, Vec<u8>)>> {
        // Peek: is the next message a FileEntry or BatchDone?
        let mut len_buf = [0u8; 4];
        match self.stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => anyhow::bail!(e),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;

        // Check if BatchDone
        if let Ok(_done) = serde_json::from_slice::<BatchDone>(&buf) {
            return Ok(None);
        }

        // Otherwise parse as FileEntry
        let entry: FileEntry = serde_json::from_slice(&buf)?;
        let ack = Ack { ok: true };
        send_json(&mut self.stream, &ack).await?;

        // Read all binary chunks for this file
        let mut data = Vec::new();
        loop {
            let mut hdr = [0u8; HDR_SIZE];
            match self.stream.read_exact(&mut hdr).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => anyhow::bail!(e),
            }
            let (ty, _idx, _off, size, _hash) = parse_header(&hdr);
            if ty == DONE_TYPE {
                break;
            }
            if ty != CHUNK_TYPE {
                anyhow::bail!("unknown chunk type: {}", ty);
            }
            let mut chunk = vec![0u8; size as usize];
            self.stream.read_exact(&mut chunk).await?;
            data.extend_from_slice(&chunk);
        }

        self.bytes_received += data.len() as u64;

        // Decompress if the file was compressed on the sender side
        if entry.compressed {
            data = crate::compress::decompress(&data)?;
        }

        Ok(Some((entry.relative_path, data)))
    }

    pub fn bytes_received(&self) -> u64 { self.bytes_received }
}

// ---------------------------------------------------------------------------
// Collect files from a directory
// ---------------------------------------------------------------------------

pub fn collect_files(root: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let mut files = Vec::new();
    if !root.is_dir() {
        anyhow::bail!("not a directory: {}", root.display());
    }
    collect_dir(root, root, &mut files)?;
    Ok(files)
}

fn collect_dir(base: &Path, dir: &Path, files: &mut Vec<(PathBuf, u64)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Skip symlinks to prevent infinite recursion
        if entry.file_type()?.is_symlink() {
            continue;
        }
        if path.is_dir() {
            collect_dir(base, &path, files)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(base).unwrap().to_path_buf();
            let size = entry.metadata()?.len();
            files.push((relative, size));
        }
    }
    Ok(())
}
