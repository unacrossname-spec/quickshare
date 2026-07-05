use sha2::{Digest, Sha256};
use std::io;

/// Split a reader into chunks, computing a SHA-256 hash per chunk.
pub struct ChunkReader<R> {
    inner: R,
    chunk_size: usize,
    index: u64,
    offset: u64,
    done: bool,
}

impl<R: io::Read> ChunkReader<R> {
    pub fn new(inner: R, chunk_size: usize) -> Self {
        Self {
            inner,
            chunk_size,
            index: 0,
            offset: 0,
            done: false,
        }
    }
}

impl<R: io::Read> Iterator for ChunkReader<R> {
    type Item = io::Result<Chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let mut buf = vec![0u8; self.chunk_size];
        let mut total = 0usize;

        loop {
            match self.inner.read(&mut buf[total..]) {
                Ok(0) if total == 0 => {
                    self.done = true;
                    return None;
                }
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if total >= self.chunk_size {
                        break;
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Some(Err(e)),
            }
        }

        buf.truncate(total);
        let hash = Sha256::digest(&buf);

        let chunk = Chunk {
            index: self.index,
            offset: self.offset,
            data: buf,
            hash: hash.into(),
        };

        self.index += 1;
        self.offset += total as u64;
        Some(Ok(chunk))
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: u64,
    pub offset: u64,
    pub data: Vec<u8>,
    pub hash: [u8; 32],
}

impl Chunk {
    pub fn verify(&self) -> bool {
        let computed = Sha256::digest(&self.data);
        computed.as_slice() == self.hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_reader_small() {
        let data = b"hello world";
        let reader = ChunkReader::new(&data[..], 4);
        let chunks: Vec<_> = reader.filter_map(|r| r.ok()).collect();
        assert_eq!(chunks.len(), 3); // 4 + 4 + 3
        assert_eq!(chunks[0].data, b"hell");
        assert_eq!(chunks[1].data, b"o wo");
        assert_eq!(chunks[2].data, b"rld");
        for c in &chunks {
            assert!(c.verify());
        }
    }

    #[test]
    fn test_chunk_reader_exact() {
        let data = b"12345678";
        let reader = ChunkReader::new(&data[..], 4);
        let chunks: Vec<_> = reader.filter_map(|r| r.ok()).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].data, b"1234");
        assert_eq!(chunks[1].data, b"5678");
    }
}
