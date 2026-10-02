//! The server's side of HTTP/3 for the loopback fakes (phase 2 M7 design
//! 4, 5.5): SETTINGS on a control stream of its own, the client's streams
//! read, requests taken apart and answered. Its frames and its QPACK are
//! code of its own — only the static table and the Huffman code are the
//! client's — so what the client sends is read by code that is not the
//! client's. It reads what this client sends and no more: field sections
//! without a dynamic table, and no Huffman-encoded string (the client
//! sends none). Never used by production code.

use super::huffman::CODES;
use super::qpack::STATIC_TABLE;
use quinn::{Connection, RecvStream, SendStream, VarInt};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

// the wire as the fake knows it
const DATA: u64 = 0x00;
const HEADERS: u64 = 0x01;
const SETTINGS: u64 = 0x04;
const GOAWAY: u64 = 0x07;
const CONTROL: u64 = 0x00;

/// An HTTP/3 server on an accepted connection. Dropping it stops reading
/// the client's streams; the connection stays the caller's.
pub struct H3Server {
    quic: Connection,
    /// Our control stream, once SETTINGS went out on it.
    control: Option<SendStream>,
    client: watch::Receiver<ClientStreams>,
    reader: JoinHandle<()>,
}

/// What the client's unidirectional streams brought.
#[derive(Default)]
struct ClientStreams {
    /// Their types, in the order they came.
    opened: Vec<u64>,
    /// The types of those the client ended.
    ended: Vec<u64>,
    /// The client's SETTINGS, once they came.
    settings: Option<Vec<(u64, u64)>>,
}

impl H3Server {
    /// HTTP/3 on `connection` without SETTINGS yet (`send_settings`): the
    /// client's unidirectional streams are read from now on.
    pub fn new(connection: Connection) -> H3Server {
        let (seen, client) = watch::channel(ClientStreams::default());
        let reader = tokio::spawn(read_client(connection.clone(), Arc::new(seen)));
        H3Server {
            quic: connection,
            control: None,
            client,
            reader,
        }
    }

    /// HTTP/3 on `connection`, `settings` sent at once.
    pub async fn start(connection: Connection, settings: &[(u64, u64)]) -> io::Result<H3Server> {
        let mut server = H3Server::new(connection);
        server.send_settings(settings).await?;
        Ok(server)
    }

    /// Opens our control stream with a SETTINGS frame of `settings`, in
    /// order.
    pub async fn send_settings(&mut self, settings: &[(u64, u64)]) -> io::Result<()> {
        let mut payload = Vec::new();
        for &(id, value) in settings {
            payload.extend(varint(id));
            payload.extend(varint(value));
        }
        let mut bytes = varint(CONTROL);
        bytes.extend(frame(SETTINGS, &payload));
        let mut control = self.quic.open_uni().await?;
        control.write_all(&bytes).await?;
        self.control = Some(control);
        Ok(())
    }

    /// A frame on our control stream, after the SETTINGS.
    pub async fn control_frame(&mut self, ty: u64, payload: &[u8]) -> io::Result<()> {
        let control = self.control.as_mut().expect("the SETTINGS went out first");
        control.write_all(&frame(ty, payload)).await?;
        Ok(())
    }

    /// GOAWAY: no request on stream `id` or later is served.
    pub async fn goaway(&mut self, id: u64) -> io::Result<()> {
        self.control_frame(GOAWAY, &varint(id)).await
    }

    /// The client's SETTINGS, once they came; an error when the connection
    /// ended first.
    pub async fn client_settings(&self) -> io::Result<Vec<(u64, u64)>> {
        let mut client = self.client.clone();
        let streams = client
            .wait_for(|streams| streams.settings.is_some())
            .await
            .map_err(|_| io::Error::other("the connection ended before the client's SETTINGS"))?;
        Ok(streams.settings.clone().unwrap_or_default())
    }

    /// The types of the unidirectional streams the client opened so far.
    pub fn client_streams(&self) -> Vec<u64> {
        self.client.borrow().opened.clone()
    }

    /// The types of the client's unidirectional streams that it ended.
    pub fn client_streams_ended(&self) -> Vec<u64> {
        self.client.borrow().ended.clone()
    }

    /// The client's next bidirectional stream; `None` once the connection
    /// is gone.
    pub async fn accept(&self) -> Option<ServerStream> {
        let (send, recv) = self.quic.accept_bi().await.ok()?;
        Some(ServerStream {
            send,
            incoming: Incoming {
                recv,
                buf: Vec::new(),
            },
        })
    }
}

impl Drop for H3Server {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Reads the client's unidirectional streams, each in a task of its own.
async fn read_client(quic: Connection, seen: Arc<watch::Sender<ClientStreams>>) {
    let mut streams = JoinSet::new();
    while let Ok(recv) = quic.accept_uni().await {
        streams.spawn(read_client_stream(recv, seen.clone()));
    }
}

/// One of the client's unidirectional streams: its type, the SETTINGS of
/// a control stream, and then whatever comes, dropped, until it ends.
async fn read_client_stream(recv: RecvStream, seen: Arc<watch::Sender<ClientStreams>>) {
    let mut incoming = Incoming {
        recv,
        buf: Vec::new(),
    };
    let Ok(Some(ty)) = incoming.varint().await else {
        return;
    };
    seen.send_modify(|streams| streams.opened.push(ty));
    if ty == CONTROL
        && let Ok(Some((SETTINGS, payload))) = incoming.frame().await
        && let Some(pairs) = settings(&payload)
    {
        seen.send_modify(|streams| streams.settings = Some(pairs));
    }
    loop {
        match incoming.recv.read_chunk(usize::MAX, true).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                seen.send_modify(|streams| streams.ended.push(ty));
                return;
            }
            Err(_) => return,
        }
    }
}

/// The pairs of a SETTINGS payload.
fn settings(mut payload: &[u8]) -> Option<Vec<(u64, u64)>> {
    let mut pairs = Vec::new();
    while !payload.is_empty() {
        let (id, n) = parse_varint(payload)?;
        let (value, m) = parse_varint(&payload[n..])?;
        payload = &payload[n + m..];
        pairs.push((id, value));
    }
    Some(pairs)
}

/// One of the client's bidirectional streams, as the fake serves it: HTTP/3
/// (`read_headers`, `send_headers`, …) or — after `peek_varint` — another
/// protocol's bytes (`AsyncRead`, `AsyncWrite`).
pub struct ServerStream {
    send: SendStream,
    incoming: Incoming,
}

impl ServerStream {
    /// The stream's first variable-length integer, left unread: a frame
    /// type (HEADERS, 0x01) for HTTP/3, 0x401 for a Hysteria 2 TCP request.
    pub async fn peek_varint(&mut self) -> io::Result<u64> {
        if !self.incoming.fill(1).await? {
            return Err(cut_short());
        }
        let len = 1 << (self.incoming.buf[0] >> 6);
        if !self.incoming.fill(len).await? {
            return Err(cut_short());
        }
        Ok(parse_varint(&self.incoming.buf)
            .expect("the whole integer")
            .0)
    }

    /// The fields of the next HEADERS frame, in order; frames of other
    /// types before it are passed over.
    pub async fn read_headers(&mut self) -> io::Result<Vec<(String, String)>> {
        loop {
            match self.incoming.frame().await? {
                Some((HEADERS, payload)) => return fields(&payload),
                Some((DATA, _)) => return Err(invalid("DATA where HEADERS was due")),
                Some(_) => {}
                None => return Err(cut_short()),
            }
        }
    }

    /// The request's content: its DATA frames up to the end of the stream.
    pub async fn read_body(&mut self) -> io::Result<Vec<u8>> {
        let mut body = Vec::new();
        while let Some((ty, payload)) = self.incoming.frame().await? {
            match ty {
                DATA => body.extend(payload),
                HEADERS => return Err(invalid("trailers: the client sends none")),
                _ => {}
            }
        }
        Ok(body)
    }

    /// The next frame, whole — type and payload — as the client framed it;
    /// `None` at the end of the stream.
    pub async fn read_frame(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        self.incoming.frame().await
    }

    /// A response head: `:status`, then `headers` (`response_section`).
    pub async fn send_headers(&mut self, status: u16, headers: &[(&str, &str)]) -> io::Result<()> {
        self.send_frame(HEADERS, &response_section(status, headers))
            .await
    }

    /// A DATA frame of `data`.
    pub async fn send_data(&mut self, data: &[u8]) -> io::Result<()> {
        self.send_frame(DATA, data).await
    }

    /// A frame of any type.
    pub async fn send_frame(&mut self, ty: u64, payload: &[u8]) -> io::Result<()> {
        self.send.write_all(&frame(ty, payload)).await?;
        Ok(())
    }

    /// Ends the server's side (FIN).
    pub fn finish(&mut self) {
        let _ = self.send.finish();
    }

    /// Resets the server's side with `code`.
    pub fn reset(&mut self, code: u64) {
        let _ = self
            .send
            .reset(VarInt::from_u64(code).expect("a QUIC integer"));
    }

    /// The code of the client's STOP_SENDING; `None` when it read the
    /// server's side to its end.
    pub async fn client_stopped(&self) -> Option<u64> {
        self.send.stopped().await.ok().flatten().map(u64::from)
    }

    /// The code of the client's RESET_STREAM; `None` when it finished.
    pub async fn client_reset(&mut self) -> Option<u64> {
        self.incoming
            .recv
            .received_reset()
            .await
            .ok()
            .flatten()
            .map(u64::from)
    }
}

impl AsyncRead for ServerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.incoming.buf.is_empty() {
            return AsyncRead::poll_read(Pin::new(&mut this.incoming.recv), cx, buf);
        }
        // what a peek read ahead comes first
        let n = this.incoming.buf.len().min(buf.remaining());
        buf.put_slice(&this.incoming.buf[..n]);
        this.incoming.buf.drain(..n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ServerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().send), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().send), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().send), cx)
    }
}

/// A stream read as the fake reads it: as much as it needs at a time.
struct Incoming {
    recv: RecvStream,
    /// Read and not yet taken.
    buf: Vec<u8>,
}

impl Incoming {
    /// At least `n` bytes read ahead; `false` when the stream ended first.
    async fn fill(&mut self, n: usize) -> io::Result<bool> {
        while self.buf.len() < n {
            match self.recv.read_chunk(n - self.buf.len(), true).await? {
                Some(chunk) => self.buf.extend_from_slice(&chunk.bytes),
                None => return Ok(false),
            }
        }
        Ok(true)
    }

    /// The next variable-length integer; `None` at the end of the stream.
    async fn varint(&mut self) -> io::Result<Option<u64>> {
        if !self.fill(1).await? {
            return Ok(None);
        }
        let len = 1 << (self.buf[0] >> 6);
        if !self.fill(len).await? {
            return Err(cut_short());
        }
        let (value, _) = parse_varint(&self.buf).expect("the whole integer");
        self.buf.drain(..len);
        Ok(Some(value))
    }

    /// The next frame whole: its type and payload; `None` at the end of the
    /// stream.
    async fn frame(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        let Some(ty) = self.varint().await? else {
            return Ok(None);
        };
        let len = self.varint().await?.ok_or_else(cut_short)?;
        let len = usize::try_from(len).map_err(|_| invalid("a frame longer than memory"))?;
        if !self.fill(len).await? {
            return Err(cut_short());
        }
        Ok(Some((ty, self.buf.drain(..len).collect())))
    }
}

/// The variable-length integer at the start of `bytes` and its length;
/// `None` when `bytes` holds part of it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1 << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let mut value = u64::from(bytes[0] & 0x3f);
    for &byte in &bytes[1..] {
        value = (value << 8) | u64::from(byte);
    }
    Some((value, len))
}

/// `value` as a variable-length integer, as short as it goes.
fn varint(value: u64) -> Vec<u8> {
    if value < 1 << 6 {
        vec![value as u8]
    } else if value < 1 << 14 {
        (0x4000 | value as u16).to_be_bytes().to_vec()
    } else if value < 1 << 30 {
        (0x8000_0000 | value as u32).to_be_bytes().to_vec()
    } else {
        (0xc000_0000_0000_0000 | value).to_be_bytes().to_vec()
    }
}

fn frame(ty: u64, payload: &[u8]) -> Vec<u8> {
    let mut bytes = varint(ty);
    bytes.extend(varint(payload.len() as u64));
    bytes.extend_from_slice(payload);
    bytes
}

/// The fields of a field section from the client, in order: static
/// entries and literals only, no Huffman-encoded string.
fn fields(section: &[u8]) -> io::Result<Vec<(String, String)>> {
    // Required Insert Count 0 and Base 0, as the client always writes them
    let Some(mut src) = section.strip_prefix(&[0, 0]) else {
        return Err(invalid("a field section that needs a dynamic table"));
    };
    let mut fields = Vec::new();
    while let Some(&first) = src.first() {
        fields.push(match first {
            // indexed, static
            0xc0..=0xff => {
                let (name, value) = static_entry(prefixed(&mut src, 6)?)?;
                (name.to_string(), value.to_string())
            }
            // a literal value with a static name, never indexed or not
            0x50..=0x5f | 0x70..=0x7f => {
                let (name, _) = static_entry(prefixed(&mut src, 4)?)?;
                (name.to_string(), literal(&mut src, 7)?)
            }
            // a literal name and value
            0x20..=0x3f => {
                let name = literal(&mut src, 3)?;
                (name, literal(&mut src, 7)?)
            }
            _ => {
                return Err(invalid(format!(
                    "field line {first:#04x}: the dynamic table"
                )));
            }
        });
    }
    Ok(fields)
}

/// An integer with a `bits`-bit prefix (RFC 7541 5.1).
fn prefixed(src: &mut &[u8], bits: u32) -> io::Result<usize> {
    let mask = (1usize << bits) - 1;
    let (&first, rest) = src.split_first().ok_or_else(cut_short)?;
    *src = rest;
    let mut value = usize::from(first) & mask;
    if value == mask {
        let mut shift = 0;
        loop {
            let (&byte, rest) = src.split_first().ok_or_else(cut_short)?;
            *src = rest;
            value += usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return Err(invalid("an integer past what a test sends"));
            }
        }
    }
    Ok(value)
}

/// A string literal whose length has a `bits`-bit prefix, the H bit right
/// above it.
fn literal(src: &mut &[u8], bits: u32) -> io::Result<String> {
    if src.first().is_some_and(|&b| b & (1 << bits) != 0) {
        return Err(invalid("a Huffman-encoded string: the client sends none"));
    }
    let len = prefixed(src, bits)?;
    if len > src.len() {
        return Err(cut_short());
    }
    let (bytes, rest) = src.split_at(len);
    *src = rest;
    String::from_utf8(bytes.to_vec()).map_err(|_| invalid("a string that is no UTF-8"))
}

fn static_entry(index: usize) -> io::Result<(&'static str, &'static str)> {
    STATIC_TABLE
        .get(index)
        .copied()
        .ok_or_else(|| invalid("an index past the static table"))
}

/// A response's field section as a server's encoder might make it:
/// `:status` and each field as an exact static entry when there is one,
/// else as a literal — with the static name when there is one — whose
/// strings are Huffman-encoded.
pub fn response_section(status: u16, headers: &[(&str, &str)]) -> Vec<u8> {
    let status = status.to_string();
    let mut out = vec![0, 0];
    for (name, value) in
        std::iter::once((":status", status.as_str())).chain(headers.iter().copied())
    {
        if let Some(index) = STATIC_TABLE
            .iter()
            .position(|&entry| entry == (name, value))
        {
            put_prefixed(&mut out, 0xc0, 6, index);
            continue;
        }
        match STATIC_TABLE.iter().position(|&(n, _)| n == name) {
            Some(index) => put_prefixed(&mut out, 0x50, 4, index),
            None => put_huffman(&mut out, 0x20, 3, name.as_bytes()),
        }
        put_huffman(&mut out, 0x00, 7, value.as_bytes());
    }
    out
}

fn put_prefixed(out: &mut Vec<u8>, flags: u8, bits: u32, value: usize) {
    let mask = (1usize << bits) - 1;
    if value < mask {
        out.push(flags | value as u8);
        return;
    }
    out.push(flags | mask as u8);
    let mut rest = value - mask;
    while rest > 0x7f {
        out.push(0x80 | (rest & 0x7f) as u8);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// `bytes` Huffman-encoded as a string literal: H, the bit above the
/// `bits`-bit prefix of the length, set.
fn put_huffman(out: &mut Vec<u8>, flags: u8, bits: u32, bytes: &[u8]) {
    let encoded = huffman_encode(bytes);
    put_prefixed(out, flags | (1 << bits), bits, encoded.len());
    out.extend_from_slice(&encoded);
}

/// `bytes` Huffman-encoded (RFC 7541 5.2): the codes of `CODES`, padded to
/// the byte with the first bits of EOS.
pub(crate) fn huffman_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u64, 0u32);
    for &byte in bytes {
        let (code, len) = CODES[usize::from(byte)];
        acc = (acc << len) | u64::from(code);
        bits += u32::from(len);
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (8 - bits)) as u8) | (0xff >> bits));
    }
    out
}

fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "h3 fake: the stream ended early",
    )
}

fn invalid(what: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("h3 fake: {what}"))
}
