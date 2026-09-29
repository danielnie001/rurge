//! A SOCKS5 server as small as a test needs (no authentication, CONNECT
//! only) that rurge starts as an `external` policy's program. It listens on
//! 127.0.0.1 only and connects nowhere but where its client asks.
//!
//! socks-helper --port <p> [--record <file>] [--delay-ms <n>] [--serve <n>]
//!              [--child <file>]
//! socks-helper --hold <file>
//!
//! `--record` appends its process id, its arguments and the proxy variables
//! it was given to `<file>`; `--delay-ms` waits before listening; `--serve`
//! serves one client at a time and exits after that many relayed sessions (a
//! connection given up before its request does not count); `--child` starts
//! a copy of itself in
//! `--hold` mode, which listens on a port of its own, writes the port to
//! `<file>` and runs until ended.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(file) = value(&args, "--hold") {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // written whole at once, so a reader never sees half a number
        let tmp = format!("{file}.tmp");
        std::fs::write(&tmp, port.to_string()).unwrap();
        std::fs::rename(&tmp, &file).unwrap();
        for stream in listener.incoming() {
            drop(stream);
        }
        return;
    }
    let port: u16 = value(&args, "--port")
        .and_then(|p| p.parse().ok())
        .expect("--port <port>");
    if let Some(file) = value(&args, "--record") {
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap();
        let mut line = format!("pid={} args={}", std::process::id(), args.join(" "));
        for name in ["HTTP_PROXY", "ALL_PROXY", "NO_PROXY", "no_proxy"] {
            line.push_str(&format!(
                " {name}={}",
                std::env::var(name).unwrap_or_default()
            ));
        }
        writeln!(out, "{line}").unwrap();
    }
    if let Some(ms) = value(&args, "--delay-ms").and_then(|v| v.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let serve: Option<usize> = value(&args, "--serve").and_then(|v| v.parse().ok());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    println!("socks-helper listening on 127.0.0.1:{port}");
    // started once listening: by then rurge has long taken this process in
    // (a child started in the first instants of a program would escape a
    // Windows Job Object)
    if let Some(file) = value(&args, "--child") {
        start_child(&file);
    }
    if let Some(limit) = serve {
        let mut relayed = 0;
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if serve_one(stream).unwrap_or(false) {
                relayed += 1;
                if relayed >= limit {
                    return;
                }
            }
        }
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || {
            let _ = serve_one(stream);
        });
    }
}

/// A copy of this program in `--hold` mode, left running: only the end of
/// the whole tree ends it, which is what the tests check.
#[allow(clippy::zombie_processes)]
fn start_child(file: &str) {
    let exe = std::env::current_exe().unwrap();
    Command::new(exe)
        .args(["--hold", file])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
}

/// `true` once a session has been relayed to its end.
fn serve_one(mut client: TcpStream) -> std::io::Result<bool> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head)?;
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods)?;
    client.write_all(&[5, 0])?;
    let mut request = [0u8; 4];
    client.read_exact(&mut request)?;
    let host: Vec<SocketAddr> = match request[3] {
        1 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip)?;
            vec![SocketAddr::from((Ipv4Addr::from(ip), port(&mut client)?))]
        }
        4 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip)?;
            vec![SocketAddr::from((Ipv6Addr::from(ip), port(&mut client)?))]
        }
        _ => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len)?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name)?;
            let port = port(&mut client)?;
            (String::from_utf8_lossy(&name).as_ref(), port)
                .to_socket_addrs()?
                .collect()
        }
    };
    let Ok(upstream) = TcpStream::connect(&host[..]) else {
        client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])?;
        return Ok(false);
    };
    client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])?;
    let (mut c2, mut u2) = (client.try_clone()?, upstream.try_clone()?);
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut u2);
        let _ = u2.shutdown(Shutdown::Write);
    });
    let (mut client, mut upstream) = (client, upstream);
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = up.join();
    Ok(true)
}

fn port(stream: &mut TcpStream) -> std::io::Result<u16> {
    let mut port = [0u8; 2];
    stream.read_exact(&mut port)?;
    Ok(u16::from_be_bytes(port))
}
