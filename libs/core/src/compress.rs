/// Compress data using LZ4.
///
/// When the `compress` feature is disabled, this is a no-op that returns
/// a copy of the input (transfers proceed uncompressed).
#[cfg(feature = "compress")]
pub fn compress(data: &[u8]) -> Vec<u8> {
    // Skip compression for tiny data (LZ4 header overhead isn't worth it)
    // or if compression doesn't actually shrink the data.
    let compressed = lz4_flex::compress_prepend_size(data);
    if compressed.len() < data.len() {
        compressed
    } else {
        data.to_vec()
    }
}

/// No-op compression when feature is disabled.
#[cfg(not(feature = "compress"))]
pub fn compress(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}

/// Decompress LZ4-compressed data.
///
/// When the `compress` feature is disabled, returns an error if the data
/// was marked as compressed (prevents silent data corruption across
/// differently-built nodes).
#[cfg(feature = "compress")]
pub fn decompress(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(lz4_flex::decompress_size_prepended(data)?)
}

/// No-op / error when feature is disabled.
#[cfg(not(feature = "compress"))]
pub fn decompress(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!(
        "received compressed data but compression support is not compiled in \
         (rebuild with default features to enable)"
    )
}

/// Returns `true` if the `compress` feature is available at compile time.
pub fn is_available() -> bool {
    cfg!(feature = "compress")
}
