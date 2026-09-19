//! Reads an HTTP-style head off a stream.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

fn end_of_head(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Reads up to and including the first empty line. Returns the head and
/// whatever was read past it (those bytes belong to what follows). A head
/// larger than `limit` bytes is `InvalidData`.
pub async fn read_head<S: AsyncRead + Unpin>(
    stream: &mut S,
    limit: usize,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        if let Some(end) = end_of_head(&buf) {
            let rest = buf.split_off(end);
            return Ok((buf, rest));
        }
        if buf.len() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("header larger than {limit} bytes"),
            ));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before the end of the header",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn the_head_ends_at_the_empty_line_and_the_rest_is_handed_back() {
        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            // split in awkward places on purpose
            for chunk in ["HTTP/1.1 200 OK\r", "\nX-A: 1\r\n\r", "\nTUNNEL", " BYTES"] {
                server.write_all(chunk.as_bytes()).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let (head, rest) = read_head(&mut client, 1024).await.unwrap();
        assert_eq!(head, b"HTTP/1.1 200 OK\r\nX-A: 1\r\n\r\n");
        assert!(b"TUNNEL BYTES".starts_with(&rest), "{rest:?}");
    }

    #[tokio::test]
    async fn oversized_and_truncated_heads_are_errors() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _ = server.write_all(&[b'x'; 3000]).await;
        });
        let err = read_head(&mut client, 1024).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "header larger than 1024 bytes");

        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            server.write_all(b"HTTP/1.1 200 OK\r\n").await.unwrap();
        });
        let err = read_head(&mut client, 1024).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
