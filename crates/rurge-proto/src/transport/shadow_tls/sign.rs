//! Shadow TLS v3 wants the last 4 bytes of the ClientHello's session id to be
//! an HMAC over that very ClientHello — and the session id is part of the
//! handshake transcript, so it cannot be patched after rustls wrote it.
//!
//! Everything random in a rustls ClientHello comes from two replaceable
//! places: `CryptoProvider::secure_random` (the client random, the session
//! id, the seed that shuffles the extensions) and `SupportedKxGroup::start`
//! (the key share). So the hello is built twice, synchronously, on one
//! thread (M2 design, appendix A):
//!
//! 1. pass 1 records every random draw on a tape and parks the real key
//!    exchange, handing the throw-away connection the public half only;
//! 2. the HMAC over hello #1 is patched into the tape, where the session id was drawn;
//! 3. pass 2 replays the tape and takes the parked key exchange.
//!
//! Hello #2 is hello #1 but for those 4 bytes, so the HMAC holds; and it is
//! the real connection's own hello, so its transcript is consistent. Four
//! checks guard the assumptions; when one fails nothing is sent at all.

use super::auth::{Chain, TAG};
use super::record::{HANDSHAKE, HEADER};
use rustls::crypto::{
    ActiveKeyExchange, CryptoProvider, GetRandomFailed, SecureRandom, SharedSecret,
    SupportedKxGroup,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, NamedGroup};
use std::cell::RefCell;
use std::sync::Arc;

const CLIENT_HELLO: u8 = 1;
const SESSION_ID_LEN: usize = 32;
/// Where the length byte of the session id sits in the record: behind the
/// record header, the handshake header (4), the version (2) and the random (32).
const SESSION_ID_LEN_AT: usize = HEADER + 4 + 2 + 32;
const SESSION_ID_AT: usize = SESSION_ID_LEN_AT + 1;
const TAG_AT: usize = SESSION_ID_AT + SESSION_ID_LEN - TAG;

enum Mode {
    Off,
    Record,
    Replay,
}

struct Script {
    mode: Mode,
    tape: Vec<u8>,
    cursor: usize,
    parked: Option<Box<dyn ActiveKeyExchange>>,
}

impl Script {
    const fn off() -> Script {
        Script {
            mode: Mode::Off,
            tape: Vec::new(),
            cursor: 0,
            parked: None,
        }
    }
}

thread_local! {
    static SCRIPT: RefCell<Script> = const { RefCell::new(Script::off()) };
}

/// Switches the script off again however `signed_hello` is left.
struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        SCRIPT.with(|s| *s.borrow_mut() = Script::off());
    }
}

fn system_random(buf: &mut [u8]) -> Result<(), GetRandomFailed> {
    // what rustls' own ring provider draws from
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), buf)
        .map_err(|_| GetRandomFailed)
}

#[derive(Debug)]
struct ScriptedRandom;

impl SecureRandom for ScriptedRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        SCRIPT.with(|s| {
            let mut s = s.borrow_mut();
            match s.mode {
                Mode::Off => system_random(buf),
                Mode::Record => {
                    system_random(buf)?;
                    s.tape.extend_from_slice(buf);
                    Ok(())
                }
                Mode::Replay => {
                    let end = s.cursor + buf.len();
                    let Some(drawn) = s.tape.get(s.cursor..end) else {
                        return Err(GetRandomFailed);
                    };
                    buf.copy_from_slice(drawn);
                    s.cursor = end;
                    Ok(())
                }
            }
        })
    }
}

/// What pass 1 hands to its throw-away connection.
struct PublicHalf {
    public: Vec<u8>,
    group: NamedGroup,
}

impl ActiveKeyExchange for PublicHalf {
    fn complete(self: Box<Self>, _peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
        Err(rustls::Error::General(
            "a recorded key exchange is never completed".into(),
        ))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        self.group
    }
}

#[derive(Debug)]
struct ScriptedX25519;

impl SupportedKxGroup for ScriptedX25519 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
        let real = rustls::crypto::ring::kx_group::X25519;
        SCRIPT.with(|s| {
            let mut s = s.borrow_mut();
            match s.mode {
                Mode::Off => real.start(),
                Mode::Record => {
                    let started = real.start()?;
                    let public = PublicHalf {
                        public: started.pub_key().to_vec(),
                        group: started.group(),
                    };
                    s.parked = Some(started);
                    Ok(Box::new(public) as Box<dyn ActiveKeyExchange>)
                }
                Mode::Replay => s
                    .parked
                    .take()
                    .ok_or_else(|| rustls::Error::General("no recorded key exchange".into())),
            }
        })
    }

    fn name(&self) -> NamedGroup {
        rustls::crypto::ring::kx_group::X25519.name()
    }
}

static RANDOM: ScriptedRandom = ScriptedRandom;
static X25519: ScriptedX25519 = ScriptedX25519;

/// The ring provider with the two scripted pieces: only for the camouflage
/// handshake of Shadow TLS v3. While no script runs it behaves like ring's
/// own, except that X25519 is the only key exchange group on offer.
pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(CryptoProvider {
        kx_groups: vec![&X25519],
        secure_random: &RANDOM,
        ..rustls::crypto::ring::default_provider()
    })
}

fn first_flight(conn: &mut ClientConnection) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    conn.write_tls(&mut out).ok()?;
    Some(out)
}

/// Exactly one handshake record holding a ClientHello with a 32-byte session id.
fn well_formed(hello: &[u8]) -> bool {
    hello.len() >= SESSION_ID_AT + SESSION_ID_LEN
        && hello[0] == HANDSHAKE
        && usize::from(u16::from_be_bytes([hello[3], hello[4]])) + HEADER == hello.len()
        && hello[HEADER] == CLIENT_HELLO
        && usize::from(hello[SESSION_ID_LEN_AT]) == SESSION_ID_LEN
}

/// HMAC-SHA1 under the password over the ClientHello without its record
/// header and with the 4 bytes of the tag as zeroes.
pub(crate) fn hello_tag(password: &[u8], hello: &[u8]) -> [u8; TAG] {
    let mut chain = Chain::new(password, &[]);
    chain.update(&hello[HEADER..TAG_AT]);
    chain.update(&[0; TAG]);
    chain.update(&hello[TAG_AT + TAG..]);
    chain.digest()
}

/// A connection whose ClientHello carries the tag, and that ClientHello
/// (already taken out of the connection: the caller sends it). `None` when
/// one of the assumptions does not hold — the caller must not connect then.
///
/// `config` must have been built on `provider()` with resumption disabled.
pub(crate) fn signed_hello(
    config: &Arc<ClientConfig>,
    name: &ServerName<'static>,
    password: &[u8],
) -> Option<(ClientConnection, Vec<u8>)> {
    let _reset = Reset;
    SCRIPT.with(|s| s.borrow_mut().mode = Mode::Record);
    let mut rehearsal = ClientConnection::new(config.clone(), name.clone()).ok()?;
    let first = first_flight(&mut rehearsal)?;
    drop(rehearsal);
    // check 1: the shape the offsets above rely on
    if !well_formed(&first) {
        return None;
    }
    let tag = hello_tag(password, &first);
    let session_id = &first[SESSION_ID_AT..SESSION_ID_AT + SESSION_ID_LEN];
    let patched = SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let mut hits = s
            .tape
            .windows(SESSION_ID_LEN)
            .enumerate()
            .filter(|(_, window)| *window == session_id)
            .map(|(at, _)| at);
        // check 2: the session id is exactly one draw
        let (Some(at), None) = (hits.next(), hits.next()) else {
            return false;
        };
        let at = at + SESSION_ID_LEN - TAG;
        s.tape[at..at + TAG].copy_from_slice(&tag);
        s.mode = Mode::Replay;
        true
    });
    if !patched {
        return None;
    }
    let mut conn = ClientConnection::new(config.clone(), name.clone()).ok()?;
    let hello = first_flight(&mut conn)?;
    // check 3: pass 2 drew exactly what pass 1 drew, and took the key exchange
    let replayed = SCRIPT.with(|s| {
        let s = s.borrow();
        s.cursor == s.tape.len() && s.parked.is_none()
    });
    // check 4: the two hellos differ in the tag and nowhere else
    let same_but_the_tag = hello.len() == first.len()
        && hello[..TAG_AT] == first[..TAG_AT]
        && hello[TAG_AT + TAG..] == first[TAG_AT + TAG..]
        && hello[TAG_AT..TAG_AT + TAG] == tag;
    (replayed && same_but_the_tag).then_some((conn, hello))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rustls::RootCertStore;

    pub(crate) fn config(roots: Arc<RootCertStore>) -> Arc<ClientConfig> {
        let mut config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.resumption = rustls::client::Resumption::disabled();
        Arc::new(config)
    }

    fn name() -> ServerName<'static> {
        ServerName::try_from("camouflage.test").unwrap()
    }

    // The four self-checks of appendix A are these tests: a rustls upgrade
    // that breaks an assumption turns them red.

    #[test]
    fn the_hello_is_one_record_with_a_32_byte_session_id_and_carries_the_tag() {
        let config = config(Arc::new(RootCertStore::empty()));
        for _ in 0..20 {
            let (_conn, hello) = signed_hello(&config, &name(), b"pw").expect("signed");
            assert!(well_formed(&hello));
            let tag = &hello[TAG_AT..TAG_AT + TAG];
            assert_eq!(tag, hello_tag(b"pw", &hello));
            assert_ne!(tag, hello_tag(b"another", &hello), "the tag is keyed");
        }
    }

    #[test]
    fn the_script_is_off_again_afterwards_and_unscripted_hellos_differ() {
        let config = config(Arc::new(RootCertStore::empty()));
        let (_a, first) = signed_hello(&config, &name(), b"pw").unwrap();
        SCRIPT.with(|s| {
            let s = s.borrow();
            assert!(matches!(s.mode, Mode::Off));
            assert!(s.tape.is_empty() && s.parked.is_none());
        });
        // no script: fresh randomness, a fresh key share
        let mut plain = ClientConnection::new(config.clone(), name()).unwrap();
        let unscripted = first_flight(&mut plain).unwrap();
        assert!(well_formed(&unscripted));
        assert_ne!(
            unscripted[HEADER + 6..HEADER + 38],
            first[HEADER + 6..HEADER + 38]
        );
        let (_b, second) = signed_hello(&config, &name(), b"pw").unwrap();
        assert_ne!(second, first, "every signed hello is fresh");
    }

    #[test]
    fn a_config_on_another_provider_is_refused_rather_than_sent_unsigned() {
        // ring's own provider never touches the tape: check 2 fails
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(RootCertStore::empty())
                .with_no_client_auth();
        assert!(signed_hello(&Arc::new(config), &name(), b"pw").is_none());
        SCRIPT.with(|s| assert!(matches!(s.borrow().mode, Mode::Off)));
    }
}
