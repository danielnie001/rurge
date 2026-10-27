//! `l2-helper`: the small program the L2 privileged tests start inside the two
//! network namespaces they make (M4 design section 14; M4c plan task 9).
//!
//! l2-helper echo <address>
//! l2-helper dns <address> <name> <ipv4>
//! l2-helper connect <address>
//! l2-helper query <server> <name>
//! l2-helper get <url> <key>
//!
//! `echo` is a TCP server that says a line back for every line it is given, and
//! `dns` a UDP DNS server that answers one name's A record with one address and
//! prints every query's source; both print `listening on <address>` once they
//! are bound, so that a test waits for them rather than sleeping. `connect`
//! sends a line to a TCP server and expects it back, `query` asks a DNS server
//! for an A record and prints the address, and `get` sends one `GET` to a
//! server on the loopback (rurge's HTTP API) with the key in `X-Key` and prints
//! the body. Every one of them ends with a non-zero status when what it came
//! for did not happen, and every line it prints is flushed at once: the test
//! reads them through a pipe.
//!
//! Only the standard library is used. The DNS messages are encoded and decoded
//! by this package's library, which is tested on every platform.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::Duration;

use rurge_privileged_tests::dns;

/// How long a read or a write of a client waits before it gives up: the tests
/// have time limits of their own, and one that comes from here says so.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The line `connect` sends and expects, and what `echo` says back.
const LINE: &str = "l2 hello\n";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match (args.first().map(String::as_str), args.len()) {
        (Some("echo"), 2) => echo(&args[1]),
        (Some("dns"), 4) => dns_server(&args[1], &args[2], &args[3]),
        (Some("connect"), 2) => connect(&args[1]),
        (Some("query"), 3) => query(&args[1], &args[2]),
        (Some("get"), 3) => get(&args[1], &args[2]),
        _ => {
            eprintln!(
                "usage: l2-helper echo <address> | dns <address> <name> <ipv4> | \
                 connect <address> | query <server> <name> | get <url> <key>"
            );
            2
        }
    };
    std::process::exit(code);
}

/// A line on standard output, flushed at once: the tests read these through a
/// pipe, and a line sitting in a buffer is a test waiting for nothing.
fn say(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// `address`, or a complaint and a failing status: the addresses come from the
/// tests, so a wrong one is a mistake to be told about and not panicked over.
fn endpoint(text: &str, default_port: u16) -> Result<SocketAddr, String> {
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(address);
    }
    format!("{text}:{default_port}")
        .parse()
        .map_err(|e| format!("`{text}` is not an address: {e}"))
}

fn ipv4(text: &str) -> Result<Ipv4Addr, String> {
    text.parse()
        .map_err(|e| format!("`{text}` is not an IPv4 address: {e}"))
}

/// `l2-helper echo <address>`: a line in, the same line out, until the client
/// goes away. Several clients at once, each on a thread of its own.
fn echo(address: &str) -> i32 {
    let listener = match TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("cannot listen on {address}: {e}");
            return 1;
        }
    };
    match listener.local_addr() {
        Ok(addr) => say(&format!("listening on {addr}")),
        Err(e) => eprintln!("cannot read back the address it listens on: {e}"),
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || echo_one(stream));
    }
    0
}

fn echo_one(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(reader);
    loop {
        let mut line = String::new();
        match lines.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                if stream.write_all(line.as_bytes()).is_err() {
                    return;
                }
                let _ = stream.flush();
            }
            Err(_) => return,
        }
    }
}

/// `l2-helper dns <address> <name> <ipv4>`: one name's A record, and every
/// query's source on standard output. A query for anything else is not
/// answered.
fn dns_server(address: &str, name: &str, resolved: &str) -> i32 {
    let address = match endpoint(address, 53) {
        Ok(address) => address,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let resolved = match ipv4(resolved) {
        Ok(address) => address,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let socket = match UdpSocket::bind(address) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("cannot listen on {address}: {e}");
            return 1;
        }
    };
    match socket.local_addr() {
        Ok(addr) => say(&format!("listening on {addr}")),
        Err(e) => eprintln!("cannot read back the address it listens on: {e}"),
    }
    let mut buf = [0u8; 1500];
    loop {
        let (read, from) = match socket.recv_from(&mut buf) {
            Ok(received) => received,
            Err(e) => {
                eprintln!("cannot read a query: {e}");
                continue;
            }
        };
        say(&format!("query from {from}"));
        let message = &buf[..read];
        let Some((asked, qtype)) = dns::name_and_type(message) else {
            eprintln!("a query that is not one: {read} bytes");
            continue;
        };
        if qtype != 1 || !asked.eq_ignore_ascii_case(name) {
            say(&format!("{asked} is not {name}; it is not answered"));
            continue;
        }
        let Some(reply) = dns::response(message, resolved) else {
            eprintln!("a query without a complete question: {read} bytes");
            continue;
        };
        if let Err(e) = socket.send_to(&reply, from) {
            eprintln!("cannot answer {from}: {e}");
        }
    }
}

/// `l2-helper connect <address>`: one line out, the same one back. The status
/// is 0 only when it came back.
fn connect(address: &str) -> i32 {
    let address = match endpoint(address, 0) {
        Ok(address) => address,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut stream = match TcpStream::connect_timeout(&address, TIMEOUT) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("cannot connect to {address}: {e}");
            return 1;
        }
    };
    if let Err(e) = open_up(&stream) {
        eprintln!("cannot set the time limits of {address}: {e}");
        return 1;
    }
    if let Err(e) = stream.write_all(LINE.as_bytes()) {
        eprintln!("cannot send to {address}: {e}");
        return 1;
    }
    let mut back = String::new();
    match BufReader::new(stream).read_line(&mut back) {
        Ok(0) => {
            eprintln!("{address} closed the connection without a line");
            return 1;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("cannot read from {address}: {e}");
            return 1;
        }
    }
    say(back.trim_end());
    if back != LINE {
        eprintln!("what came back is not what was sent: {back:?}");
        return 1;
    }
    0
}

fn open_up(stream: &TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))
}

/// `l2-helper query <server> <name>`: one A query, and the address that comes
/// back on standard output.
fn query(server: &str, name: &str) -> i32 {
    let server = match endpoint(server, 53) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("cannot open a socket of its own: {e}");
            return 1;
        }
    };
    if let Err(e) = socket.set_read_timeout(Some(TIMEOUT)) {
        eprintln!("cannot set the time limit: {e}");
        return 1;
    }
    let asked = match dns::query(0x4c32, name) {
        Ok(asked) => asked,
        Err(e) => {
            eprintln!("cannot write the query: {e}");
            return 1;
        }
    };
    if let Err(e) = socket.send_to(&asked, server) {
        eprintln!("cannot send to {server}: {e}");
        return 1;
    }
    let mut buf = [0u8; 1500];
    let read = match socket.recv_from(&mut buf) {
        Ok((read, _)) => read,
        Err(e) => {
            eprintln!("no answer from {server}: {e}");
            return 1;
        }
    };
    match dns::answer(&buf[..read]) {
        Some(address) => {
            say(&address.to_string());
            0
        }
        None => {
            eprintln!("{server} answered {name} with no address");
            1
        }
    }
}

/// `l2-helper get <url> <key>`: one `GET` to `http://<host>:<port>/<path>` with
/// `key` in `X-Key`, and the body on standard output. Anything but a 200 is a
/// failing status, with what the server said on standard error.
fn get(url: &str, key: &str) -> i32 {
    let Some(rest) = url.strip_prefix("http://") else {
        eprintln!("`{url}` is not an http:// URL");
        return 1;
    };
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let address = match endpoint(host, 80) {
        Ok(address) => address,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut stream = match TcpStream::connect_timeout(&address, TIMEOUT) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("cannot connect to {address}: {e}");
            return 1;
        }
    };
    if let Err(e) = open_up(&stream) {
        eprintln!("cannot set the time limits of {address}: {e}");
        return 1;
    }
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Key: {key}\r\nConnection: close\r\n\r\n");
    if let Err(e) = stream.write_all(request.as_bytes()) {
        eprintln!("cannot send the request to {address}: {e}");
        return 1;
    }
    let mut response = Vec::new();
    if let Err(e) = stream.read_to_end(&mut response) {
        eprintln!("cannot read the answer from {address}: {e}");
        return 1;
    }
    let (head, body) = match split(&response, b"\r\n\r\n") {
        Some(parts) => parts,
        None => {
            eprintln!(
                "{address} answered without a head: {} bytes",
                response.len()
            );
            return 1;
        }
    };
    let head = String::from_utf8_lossy(head);
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200") {
        eprintln!(
            "{address} answered `{status}`: {}",
            String::from_utf8_lossy(body)
        );
        return 1;
    }
    let lower = head.to_ascii_lowercase();
    let body = if lower.contains("transfer-encoding: chunked") {
        match dechunk(body) {
            Some(body) => body,
            None => {
                eprintln!("the answer from {address} ends inside a chunk");
                return 1;
            }
        }
    } else {
        body.to_vec()
    };
    say(&String::from_utf8_lossy(&body));
    0
}

/// `bytes` cut at the first `needle`: what is before it, and what is after.
fn split<'a>(bytes: &'a [u8], needle: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let at = bytes
        .windows(needle.len())
        .position(|window| window == needle)?;
    Some((&bytes[..at], &bytes[at + needle.len()..]))
}

/// The body of a chunked response (RFC 9112 7.1): the sizes in hexadecimal and
/// the pieces between them, up to the empty piece.
fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let (line, after) = split(rest, b"\r\n")?;
        let size = std::str::from_utf8(line).ok()?;
        let size = size.split(';').next()?.trim();
        let size = usize::from_str_radix(size, 16).ok()?;
        if size == 0 {
            return Some(body);
        }
        if after.len() < size + 2 {
            return None;
        }
        body.extend_from_slice(&after[..size]);
        rest = &after[size + 2..];
    }
}
