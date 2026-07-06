const MAGIC: &[u8; 8] = b"QSBUNDLE";
const VERSION: u32 = 1;

/// Serialize a list of (path, data) pairs into a single bundle blob.
///
/// Wire format:
///   [magic:    b"QSBUNDLE"  8 bytes]
///   [version:  u32 LE       4 bytes]
///   [count:    u32 LE       4 bytes]
///   [entries...]
///     [path_len: u32 LE     4 bytes]
///     [path:     UTF-8      path_len bytes]
///     [data_len: u64 LE     8 bytes]
///     [data:     raw        data_len bytes]
pub fn create_bundle(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (path, data) in files {
        let path_bytes = path.as_bytes();
        buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(path_bytes);
        buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
        buf.extend_from_slice(data);
    }
    buf
}

/// Deserialize a bundle blob back into a list of (path, data) pairs.
pub fn extract_bundle(data: &[u8]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let input_len = data.len();
    let mut pos = 0usize;

    fn need(pos: usize, required: usize, total: usize) -> anyhow::Result<()> {
        if pos + required > total {
            anyhow::bail!("unexpected end of bundle data");
        }
        Ok(())
    }

    // Magic
    need(pos, 8, input_len)?;
    if &data[pos..pos + 8] != MAGIC {
        anyhow::bail!("invalid bundle magic");
    }
    pos += 8;

    // Version
    need(pos, 4, input_len)?;
    let version = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
    pos += 4;
    if version != VERSION {
        anyhow::bail!("unsupported bundle version: {version}");
    }

    // Count
    need(pos, 4, input_len)?;
    let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;

    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        // Path length
        need(pos, 4, input_len)?;
        let path_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        // Path
        need(pos, path_len, input_len)?;
        let path = String::from_utf8(data[pos..pos + path_len].to_vec())
            .map_err(|e| anyhow::anyhow!("invalid UTF-8 path in bundle: {e}"))?;
        pos += path_len;

        // Data length
        need(pos, 8, input_len)?;
        let data_len = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;

        // Data
        need(pos, data_len, input_len)?;
        let file_data = data[pos..pos + data_len].to_vec();
        pos += data_len;

        files.push((path, file_data));
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let files = vec![
            ("a.txt".to_string(), b"hello".to_vec()),
            ("sub/b.txt".to_string(), b"world".to_vec().repeat(100)),
            ("empty.dat".to_string(), vec![]),
        ];
        let blob = create_bundle(&files);
        let extracted = extract_bundle(&blob).unwrap();
        assert_eq!(extracted, files);
    }

    #[test]
    fn invalid_magic() {
        assert!(extract_bundle(b"NOTBUNDLE").is_err());
    }
}
