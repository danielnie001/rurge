//! What the test files of this directory share. Each test file is a crate of
//! its own and uses a part of all this only: hence the two `allow`s.

#![allow(dead_code, unused_imports)]

pub use rurge_config::HostName;
pub use rurge_config::spec::{ExternalSpec, Secret};
pub use rurge_net::connector::{ConnectOpts, Target};
pub use rurge_proto::Outbound;
pub use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessGroup, ProcessHook};
pub use std::net::SocketAddr;
pub use std::path::{Path, PathBuf};
pub use std::sync::Arc;
pub use std::time::{Duration, Instant};
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub use tokio::net::TcpStream;

/// The test program (`src/bin/socks-helper.rs`).
pub fn helper() -> String {
    env!("CARGO_BIN_EXE_socks-helper").to_string()
}

/// A port that was free a moment ago.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `rurge-platform::process` behind the hook, as the bin wires it.
pub struct Platform;

struct Tree(rurge_platform::process::ProcessTree);

impl ProcessGroup for Tree {
    fn terminate(&mut self) -> std::io::Result<()> {
        self.0.terminate()
    }
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }
}

impl ProcessHook for Platform {
    fn prepare(&self, command: &mut std::process::Command) {
        rurge_platform::process::prepare(command);
    }
    fn contain(&self, pid: u32) -> std::io::Result<Option<Box<dyn ProcessGroup>>> {
        let tree = rurge_platform::process::ProcessTree::contain(pid)?;
        Ok(Some(Box::new(Tree(tree))))
    }
}

/// An outbound running the helper with `args`, its log in `dir`.
pub fn outbound(name: &str, args: &[String], port: u16, dir: &Path) -> ExternalOutbound {
    let spec = ExternalSpec {
        exec: helper(),
        args: Secret::new(args.to_vec()),
        local_port: port,
        addresses: Vec::new(),
        udp_relay: true,
    };
    ExternalOutbound::new(name, &spec, dir, Arc::new(Platform))
}

/// `--port <port>` and whatever else.
pub fn args(port: u16, more: &[&str]) -> Vec<String> {
    let mut out = vec!["--port".to_string(), port.to_string()];
    out.extend(more.iter().map(|s| s.to_string()));
    out
}

/// A loopback server that echoes every connection back.
pub async fn echo_server() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

pub fn target(addr: SocketAddr) -> Target {
    Target::new(HostName::parse(&addr.ip().to_string()), addr.port())
}

/// One round trip through `outbound` to the echo server at `echo`.
pub async fn round_trip(outbound: &dyn Outbound, echo: SocketAddr) -> Result<(), String> {
    let mut stream = outbound
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(&buf, b"ping");
    Ok(())
}

/// Waits (at most 10 seconds) until nothing listens on `port` any more.
pub async fn wait_closed(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let attempt = tokio::time::timeout(
            Duration::from_millis(500),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await;
        // refused, or no answer within the slot (Windows refuses slowly)
        if !matches!(attempt, Ok(Ok(_))) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "port {port} still accepts connections"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits (at most 10 seconds) for `path` to exist; returns its text.
pub async fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
