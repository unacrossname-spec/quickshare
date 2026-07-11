use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener as TokioListener, TcpStream as TokioStream};

/// Wrapper around a TCP listener.
pub struct TcpListener {
    inner: TokioListener,
}

impl TcpListener {
    /// Bind to a socket address.
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        Ok(Self {
            inner: TokioListener::bind(addr).await?,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }

    /// Accept an incoming connection.
    pub async fn accept(&self) -> Result<TcpStream> {
        let (stream, _) = self.inner.accept().await?;
        Ok(TcpStream { inner: stream })
    }
}

/// Wrapper around a TCP stream.
pub struct TcpStream {
    inner: TokioStream,
}

impl TcpStream {
    /// Construct from a raw tokio TcpStream.
    pub fn new(inner: TokioStream) -> Self {
        Self { inner }
    }

    /// Connect to a remote address.
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        Ok(Self {
            inner: TokioStream::connect(addr).await?,
        })
    }

    pub fn inner(&self) -> &TokioStream {
        &self.inner
    }

    pub fn into_inner(self) -> TokioStream {
        self.inner
    }

    pub fn peer_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.peer_addr()?)
    }
}

// Delegate AsyncRead / AsyncWrite to inner stream
impl tokio::io::AsyncRead for TcpStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for TcpStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
