//! The server side of simple-obfs (`http` / `tls`), written independently of
//! the client (`transport::obfs`) so each checks the other: a fake server
//! wraps an accepted connection with `accept_obfs` and speaks its protocol
//! over the returned stream.

use crate::transport::prefixed;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::ObfsMode;
use rurge_net::connector::BoxedStream;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// What the client's first packet said.
#[derive(Clone, Debug, Default)]
pub struct ObfsHello {
    /// The `Host` header of `http` (port included), the server name of `tls`.
    pub host: String,
    /// The request path (`http` only).
    pub uri: Option<String>,
    pub user_agent: Option<String>,
    /// The payload that rode in the first packet.
    pub first_payload: Vec<u8>,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("fake obfs: {what}"))
}

/// The largest record the reference server accepts from a client.
const MAX_RECORD: usize = 16384;

/// Reads the client's first packet and returns a stream that yields the
/// first payload and then the rest of the client's data, and whose writes
/// are camouflaged: the first one behind the upgrade answer (`http`) or the
/// fake ServerHello and ChangeCipherSpec (`tls`), the later ones raw or in
/// application data records. A shutdown before any write sends nothing.
/// A client record over 16 KiB ends the connection (as the reference server).
pub async fn accept_obfs(
    mut stream: BoxedStream,
    mode: ObfsMode,
) -> io::Result<(BoxedStream, ObfsHello)> {
    let (hello, session_id) = match mode {
        ObfsMode::Http => (read_request(&mut stream).await?, [0u8; 32]),
        ObfsMode::Tls => read_client_hello(&mut stream).await?,
    };
    let (app, pump) = tokio::io::duplex(64 * 1024);
    let (from_client, to_client) = tokio::io::split(stream);
    let (from_app, to_app) = tokio::io::split(pump);
    tokio::spawn(async move {
        let _ = tokio::join!(
            inbound(mode, from_client, to_app),
            outbound(mode, session_id, from_app, to_client),
        );
    });
    let first = hello.first_payload.clone();
    Ok((prefixed::boxed(first, Box::new(app)), hello))
}

async fn read_request(stream: &mut BoxedStream) -> io::Result<ObfsHello> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > MAX_RECORD {
            return Err(bad("the request head is too long"));
        }
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).map_err(|_| bad("the request head is not text"))?;
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let uri = request
        .strip_prefix("GET ")
        .and_then(|rest| rest.strip_suffix(" HTTP/1.1"))
        .ok_or_else(|| bad("not a GET request"))?;
    let mut hello = ObfsHello {
        uri: Some(uri.to_string()),
        ..ObfsHello::default()
    };
    let mut upgrade = false;
    let mut length = None;
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(": ").ok_or_else(|| bad("a bad header"))?;
        match name.to_ascii_lowercase().as_str() {
            "host" => hello.host = value.to_string(),
            "user-agent" => hello.user_agent = Some(value.to_string()),
            "upgrade" => upgrade = value == "websocket",
            "content-length" => length = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    if !upgrade {
        return Err(bad("no `Upgrade: websocket`"));
    }
    let length = length.ok_or_else(|| bad("no `Content-Length`"))?;
    hello.first_payload = vec![0u8; length];
    stream.read_exact(&mut hello.first_payload).await?;
    Ok(hello)
}

/// Takes `n` bytes off the front of `data`.
fn take<'a>(data: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    if data.len() < n {
        return Err(bad("the ClientHello is cut short"));
    }
    let (head, rest) = data.split_at(n);
    *data = rest;
    Ok(head)
}

fn take_u16(data: &mut &[u8]) -> io::Result<usize> {
    let b = take(data, 2)?;
    Ok(usize::from(u16::from_be_bytes([b[0], b[1]])))
}

/// The hello and the session id the ServerHello echoes.
async fn read_client_hello(stream: &mut BoxedStream) -> io::Result<(ObfsHello, [u8; 32])> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).await?;
    if header[..3] != [0x16, 0x03, 0x01] {
        return Err(bad("not a TLS handshake record"));
    }
    let mut body = vec![0u8; usize::from(u16::from_be_bytes([header[3], header[4]]))];
    stream.read_exact(&mut body).await?;
    let mut data = &body[..];
    let handshake = take(&mut data, 4)?;
    let length = usize::from(u16::from_be_bytes([handshake[2], handshake[3]]));
    if handshake[..2] != [0x01, 0x00] || length != data.len() {
        return Err(bad("not a ClientHello"));
    }
    take(&mut data, 2 + 4 + 28)?;
    if take(&mut data, 1)? != [32] {
        return Err(bad("the session id is not 32 bytes"));
    }
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(take(&mut data, 32)?);
    let suites = take_u16(&mut data)?;
    take(&mut data, suites)?;
    let methods = take(&mut data, 1)?[0];
    take(&mut data, usize::from(methods))?;
    let extensions = take_u16(&mut data)?;
    if extensions != data.len() {
        return Err(bad("the extensions do not fill the ClientHello"));
    }
    let mut hello = ObfsHello::default();
    let (mut ticket, mut name) = (false, false);
    while !data.is_empty() {
        let kind = take_u16(&mut data)?;
        let len = take_u16(&mut data)?;
        let mut ext = take(&mut data, len)?;
        match kind {
            0x0023 => {
                hello.first_payload = ext.to_vec();
                ticket = true;
            }
            0x0000 => {
                take_u16(&mut ext)?;
                if take(&mut ext, 1)? != [0] {
                    return Err(bad("the server name is not a host name"));
                }
                let len = take_u16(&mut ext)?;
                hello.host = String::from_utf8(take(&mut ext, len)?.to_vec())
                    .map_err(|_| bad("the server name is not text"))?;
                name = true;
            }
            _ => {}
        }
    }
    if !ticket || !name {
        return Err(bad("no session ticket or no server name"));
    }
    Ok((hello, session_id))
}

/// The client's data after its first packet, towards the fake's protocol.
async fn inbound(
    mode: ObfsMode,
    mut from: impl AsyncRead + Unpin,
    mut to: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    match mode {
        ObfsMode::Http => {
            tokio::io::copy(&mut from, &mut to).await?;
        }
        ObfsMode::Tls => {
            let mut body = vec![0u8; MAX_RECORD];
            loop {
                let mut header = [0u8; 5];
                match from.read_exact(&mut header).await {
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e),
                }
                let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
                if header[..3] != [0x17, 0x03, 0x03] || len > MAX_RECORD {
                    return Err(bad("a bad client record"));
                }
                from.read_exact(&mut body[..len]).await?;
                to.write_all(&body[..len]).await?;
            }
        }
    }
    to.shutdown().await
}

fn server_hello(session_id: &[u8; 32]) -> Vec<u8> {
    let mut out = vec![
        0x16, 0x03, 0x01, 0x00, 0x5b, 0x02, 0x00, 0x00, 0x57, 0x03, 0x03,
    ];
    let mut random = [0u8; 32];
    let _ = getrandom::fill(&mut random);
    out.extend_from_slice(&random);
    out.push(32);
    out.extend_from_slice(session_id);
    // ECDHE-RSA-CHACHA20-POLY1305, no compression, and the extensions
    // length left at 0 although extensions follow (as the reference does)
    out.extend_from_slice(&[0xcc, 0xa8, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0xff, 0x01, 0x00, 0x01, 0x00]);
    out.extend_from_slice(&[0x00, 0x17, 0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]);
    // ChangeCipherSpec
    out.extend_from_slice(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
    out
}

/// The fake's protocol data, camouflaged, towards the client.
async fn outbound(
    mode: ObfsMode,
    session_id: [u8; 32],
    mut from: impl AsyncRead + Unpin,
    mut to: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    let mut buf = vec![0u8; MAX_RECORD];
    let mut first = true;
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let mut packet = Vec::with_capacity(n + 256);
        match mode {
            ObfsMode::Http if first => {
                let mut accept = [0u8; 16];
                let _ = getrandom::fill(&mut accept);
                packet.extend_from_slice(
                    format!(
                        "HTTP/1.1 101 Switching Protocols\r\n\
                         Server: nginx/1.18.0\r\n\
                         Date: Wed, 30 Sep 2026 00:00:00 GMT\r\n\
                         Upgrade: websocket\r\n\
                         Connection: Upgrade\r\n\
                         Sec-WebSocket-Accept: {}\r\n\r\n",
                        STANDARD.encode(accept)
                    )
                    .as_bytes(),
                );
            }
            ObfsMode::Http => {}
            ObfsMode::Tls => {
                let kind = if first {
                    packet.extend_from_slice(&server_hello(&session_id));
                    0x16
                } else {
                    0x17
                };
                packet.extend_from_slice(&[kind, 0x03, 0x03]);
                packet.extend_from_slice(&(n as u16).to_be_bytes());
            }
        }
        packet.extend_from_slice(&buf[..n]);
        to.write_all(&packet).await?;
        first = false;
    }
    to.shutdown().await
}
