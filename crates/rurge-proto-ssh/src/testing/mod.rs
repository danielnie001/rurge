//! Keys and a loopback SSH server for the tests of this crate and of its
//! dependants (feature `testing`).
//!
//! The private keys below were made with `ssh-keygen` for this test suite
//! only; they guard nothing.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::KeystoreItem;
use rurge_config::keystore::KeystoreType;
use rurge_config::span::Span;
use russh::keys::ssh_key::LineEnding;
use std::path::Path;
use std::sync::Arc;

mod server;

pub use russh::keys::ssh_key::Algorithm;
pub use russh::keys::{PrivateKey, PublicKey};
pub use server::{FakeSsh, FakeSshOpts};

/// An RSA key (2048 bits): making one in a debug build takes too long.
pub const RSA_KEY: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABFwAAAAdzc2gtcn
NhAAAAAwEAAQAAAQEA1g1TZD0u2/ZxHgzZF13blq+aKGB4mmR/q7nmbN8vpltGszI2PV+G
vviYK7S12l4NbwRn3zJtGExapyfQ/U+dPpwTd8avQEKRG/vO+Nkv/xoVSb+Yh2bmILyWrE
HETCM/aaXQitcOUd8WwyKL1si3y18x+xCx+9rkUtKz8xU8JnMQReIxk0eucvtex4OVJQDf
BEeqMiEsn32zKRwgOrKgrX25e/jEQDKWrpRC2GKBRk5IEHrx9kPHYfDtjDT7ulztlab6R0
NZOR8dSmil/4YN9RY3FF0fMXV2BBlpJZy2kFYFV7YMmEYWy+k/in4gicXCAEPcgsOfoxvl
3dS0xSlv2wAAA8DvOb8O7zm/DgAAAAdzc2gtcnNhAAABAQDWDVNkPS7b9nEeDNkXXduWr5
ooYHiaZH+rueZs3y+mW0azMjY9X4a++JgrtLXaXg1vBGffMm0YTFqnJ9D9T50+nBN3xq9A
QpEb+8742S//GhVJv5iHZuYgvJasQcRMIz9ppdCK1w5R3xbDIovWyLfLXzH7ELH72uRS0r
PzFTwmcxBF4jGTR65y+17Hg5UlAN8ER6oyISyffbMpHCA6sqCtfbl7+MRAMpaulELYYoFG
TkgQevH2Q8dh8O2MNPu6XO2VpvpHQ1k5Hx1KaKX/hg31FjcUXR8xdXYEGWklnLaQVgVXtg
yYRhbL6T+KfiCJxcIAQ9yCw5+jG+Xd1LTFKW/bAAAAAwEAAQAAAQAURCy6F+Tg5KNvIe5H
/RX2XWfuHLwuegdwfehoNHVxfcDi5IUoKGw8lpLpyHFTXIZPFY60HjUgENKgcu+hnDEaJX
Lea0xafDL7AEtnWkDmGVUcp2xMnZx6SwDFDHEGeGvfl9h33Ma5T7L7BMFSs6xbMAcuazU+
0Em/4b0x7bfFOAFky/5T91eR0qLSvEPZb3fivbKMWYI99h77ovMficF2AKkP4NNNhgYt47
/VmFgt+qQaFNwghcv8zSQfz4oVA10jcPGHkNIJc1l+Od3tnHH0dwBX6c6yEVBZ7RUCdrCU
aGQfwH8iycz+QKDRvDpFZvhBZhwtkQ+d7pd4a+yP/Qy5AAAAgCQTHCFW2ANFGgOSQhcPDW
EWuPGM2Qz6xStCuQ0kQzPLwD/+IP8WBnO7rKlqTrcsnSYQE0igcPyXXckET08fSTzym3Xo
tG5D5Ny4+kZ6ZTzcChWg9a/dqd/hScvoA0BZLjGhAZX5jy7I3Kv+fa3rhsxqKhxwB2i94w
MmZK25IxVAAAAAgQDs8GpkMyUhLb3/U4eXo6n0bIWfeHgu0xNK1jX7pUOcJOX+2T4GQGiP
vJPjAJXgkAabCiBd6Y2s9lmhnfdlpVVmdqbIhdk+biXYbgEIBEMerA89v0BGjMViB5mccL
GGJaiUOKZ/I+T0c+eek64buaD0texTfJPntOPOsCN8xH5e7wAAAIEA50WRavAYLJkweVjf
890KdC0qvYgGibkXpe48DMyujbklsR/TUuw4Rzo0SYI5jkG8eKPK1ITFwoSg9C3KUYgJIW
41/qiFo7a95wTH0ntj9VAAYXgCilezpnBfVT66yvYrNnKpxqqYA9pFPwE/qP15ronnaoHd
fFB8G1hLZaLyvdUAAAAKcnVyZ2UtdGVzdAE=
-----END OPENSSH PRIVATE KEY-----
";

/// An Ed25519 key protected by the passphrase `pw`.
pub const ED25519_WITH_PASSPHRASE: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABBgagZIBd
zn7HTsSPluzWLtAAAAAQAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIJ6BQgWnxd54MYop
XpKyvebv6xD3l1QmX3/slN+Cnn7wAAAAkCmr6vNBiyz/Hg05H08eYsxM5hgpg9PEoxWkFH
G2rm9daIM1Th+HDfi/QfxAtnDIja0lJ6YAg1913VDICr5F25kZYznc58gDaiefgLb/V9o6
hOGIFUfm3s1e1GZ/Wg6eoFb/F/EU4YH4Zqqfkd5hehJI4VGGU9iDhYBxmgZp293DLuG89C
mICxruBuMF6RghVg==
-----END OPENSSH PRIVATE KEY-----
";

/// A DSA key (1024 bits), which rurge does not accept.
pub const DSA_KEY: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABsgAAAAdzc2gtZH
NzAAAAgQDibpJx8ewUMR81iEGJkOCSb/Y0/RClXE/LEKoctha1r9R03XT/US9pD7Y3krUg
zewzBoL5fsGmyHoe6x75+hJlTj7dAb0uYWy5cg269LG0/B8rtIHXIbd1xgrbFKWleFIsMG
8gRTbvPGlbL6Nd9vr2lq2R1iAKLeUmIuEaulOZ+wAAABUA6hnzf4B17PNL6w1MB1HjlnF6
PmcAAACARJn1yli87Zb8qA7C4ZUj+bsxZGka4Rkl769jDSbPhRB9bQRIcUrPe59A1cEi70
hazvQMVuJJtoa7S+cKUi0wFnJ5djKi/5QboaHhfjweN6Je9wKOFZqdTJrqS4UJ+1kov1lI
06B6VQkwxorBTwPXnlHWP+Anz+m6qwN2crHOADIAAACBAJDUZ8Tc2Y9z0KOXJTcCIJz/9p
4vNfjiQFoag0L2GV1wndwXjEerjcyzZrHsDgzIwF3bvnuihEhhHU2izbQwoDr8k4Z8NYhp
SpGZK6nS/pF6JWjgwz5eK1Z2+ypGEqaS36F4O96/NpXqROWoU827SUX2xzId+6WFuceO0w
25Fw2uAAAB6LeBvuq3gb7qAAAAB3NzaC1kc3MAAACBAOJuknHx7BQxHzWIQYmQ4JJv9jT9
EKVcT8sQqhy2FrWv1HTddP9RL2kPtjeStSDN7DMGgvl+wabIeh7rHvn6EmVOPt0BvS5hbL
lyDbr0sbT8Hyu0gdcht3XGCtsUpaV4UiwwbyBFNu88aVsvo132+vaWrZHWIAot5SYi4Rq6
U5n7AAAAFQDqGfN/gHXs80vrDUwHUeOWcXo+ZwAAAIBEmfXKWLztlvyoDsLhlSP5uzFkaR
rhGSXvr2MNJs+FEH1tBEhxSs97n0DVwSLvSFrO9AxW4km2hrtL5wpSLTAWcnl2MqL/lBuh
oeF+PB43ol73Ao4Vmp1MmupLhQn7WSi/WUjToHpVCTDGisFPA9eeUdY/4CfP6bqrA3Zysc
4AMgAAAIEAkNRnxNzZj3PQo5clNwIgnP/2ni81+OJAWhqDQvYZXXCd3BeMR6uNzLNmsewO
DMjAXdu+e6KESGEdTaLNtDCgOvyThnw1iGlKkZkrqdL+kXolaODDPl4rVnb7KkYSppLfoX
g73r82lepE5ahTzbtJRfbHMh37pYW5x47TDbkXDa4AAAAVAJ95JG/htpe16yIM7dVN9roj
zkgUAAAACnJ1cmdlLXRlc3QBAgMEBQYH
-----END OPENSSH PRIVATE KEY-----
";

/// A keystore item holding `key_text` the way `[Keystore]` does: the whole
/// key file, in Base64.
pub fn keystore_item(name: &str, key_text: &str) -> KeystoreItem {
    KeystoreItem {
        name: name.into(),
        kind: KeystoreType::OpensshPrivateKey,
        base64: STANDARD.encode(key_text),
        password: None,
        unknown: Vec::new(),
        span: Span::new(Arc::from(Path::new("t.conf")), 1),
    }
}

/// `key` the way a `[Keystore]` item's `base64=` holds it.
pub fn keystore_base64(key: &PrivateKey) -> String {
    STANDARD.encode(key.to_openssh(LineEnding::LF).expect("an OpenSSH key file"))
}

/// `key` the way `server-fingerprint` takes it: `<algorithm> <base64>`.
pub fn fingerprint_of(key: &PublicKey) -> String {
    format!(
        "{} {}",
        key.algorithm(),
        STANDARD.encode(key.to_bytes().expect("an encoded key"))
    )
}

/// A fresh Ed25519 or ECDSA key (for RSA, `RSA_KEY`).
pub fn random_key(algorithm: Algorithm) -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), algorithm).expect("a random key")
}
