use uuid::Uuid;

pub type DeviceId = String;
pub type TransferId = Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub listen_port: u16,
    pub max_streams: u32,
    pub chunk_size: usize,
    pub pipeline_depth: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            listen_port: 8877,
            max_streams: 8,
            chunk_size: 4 * 1024 * 1024,  // 4MB
            pipeline_depth: 4,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChunkInfo {
    pub index: u64,
    pub offset: u64,
    pub size: usize,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
    pub chunk_size: usize,
    pub chunk_count: u64,
    pub file_hash: [u8; 32],
    #[serde(default)]
    pub compressed: bool,
    #[serde(default)]
    pub bundle: bool,
    /// Each chunk is independently compressed (enables streaming).
    /// When false (default), the entire file was compressed as one blob.
    #[serde(default)]
    pub stream: bool,
    /// Data is AES-256-GCM encrypted per chunk.
    /// The encryption key is derived from a pre-shared password (never transmitted).
    #[serde(default)]
    pub encrypted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TransferEvent {
    Progress {
        id: TransferId,
        bytes_sent: u64,
        total_bytes: u64,
        speed_bps: u64,
    },
    Completed {
        id: TransferId,
    },
    Failed {
        id: TransferId,
        error: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ControlMessage {
    TransferRequest {
        transfer_id: TransferId,
        file_meta: FileMeta,
    },
    TransferAccept {
        transfer_id: TransferId,
        received_chunks: Vec<u64>,
    },
    TransferReject {
        transfer_id: TransferId,
        reason: String,
    },
    TransferDone {
        transfer_id: TransferId,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("Connection closed")]
    ConnectionClosed,
    #[error("Transfer cancelled")]
    Cancelled,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialize error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}
