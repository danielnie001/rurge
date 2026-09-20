//! Reference vectors for the VMess AEAD codec.
//!
//! Source: the functions of v2fly/v2ray-core (master, 2026-09-20)
//! `proxy/vmess/aead/{kdf,authid,encrypt}.go`,
//! `proxy/vmess/encoding/{client,auth}.go` and `common/crypto/auth.go`, copied
//! into a standard-library-only Go program and run with the fixed inputs
//! below (time 1700000000, AuthID random `01020304`, connection nonce
//! `1011121314151617`, three padding bytes `a1a2a3`). An independent Python
//! derivation written from the protocol description agrees on every value;
//! the ChaCha20-Poly1305 chunks come from that derivation alone (the Go
//! standard library has no ChaCha20-Poly1305).

use super::header::Session;

/// The manual's example id, `0233d11c-15a4-47d3-ade3-48ffca0ce119`.
pub(crate) const UUID: [u8; 16] = [
    0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1, 0x19,
];

/// body IV `20..2f`, body key `30..3f`, response check byte `5a`.
pub(crate) fn session() -> Session {
    let mut s = Session {
        body_iv: [0; 16],
        body_key: [0; 16],
        response_v: 0x5a,
    };
    for i in 0..16u8 {
        s.body_iv[usize::from(i)] = 0x20 + i;
        s.body_key[usize::from(i)] = 0x30 + i;
    }
    s
}

/// `example.com:443` as VMess writes it: port, type 2, length, name.
pub(crate) fn address() -> Vec<u8> {
    let mut a = vec![0x01, 0xbb, 2, 11];
    a.extend_from_slice(b"example.com");
    a
}

pub(crate) fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

pub(crate) const REQUEST_PLAIN: &str = "01202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f5a0533000101bb020b6578616d706c652e636f6da1a2a398a86391";
pub(crate) const REQUEST_SEALED: &str = "c65fe1f5eada535d481be2645132f215c1763ad5769a196e356ef10a09c3ffa14c57101112131415161725e6876107f5484048fdf2af4becc615e40c4263f45ffbb7f6fe2f8548076dd61eb6ebb6fedc4e422159241ad68b210e1cfe4356670d31fd44789ae62010ff33185ef9eb2a5324e114b87557";
/// `hello`, the bytes `00..27`, then the empty end-of-stream chunk.
pub(crate) const REQUEST_CHUNKS_AES: &str = "e49d4ba7ab09e5ade26eae75b50e9586a717ba52df4054220d2681b355c66ccfc7373b0e8dc50d7bc5693c83a37ab32c2d43f7a241bc94243bfed46e7722c6b84dd4339e454490a32477b51a30e553329075922c7461a107ed105859d0ac6eb92fde07";
/// The head `5a 00 00 00`.
pub(crate) const RESPONSE_SEALED: &str =
    "6797df2b7410a67de894084db988ae97c4774a80a14980ca327785428f407bc7a23a31c0db4d";
/// `world`, then the end-of-stream chunk.
pub(crate) const RESPONSE_CHUNKS_AES: &str =
    "a9ecd094dd7369ac2392c6a3e89ac65273e9e112684569a16583ef27af34548e1075794f199626737a";
pub(crate) const REQUEST_CHUNKS_CHACHA: &str = "e49d405260ed5fa743f035fe5f23d41a83f4843220f605220d973e3fb6ed8f1a62ef8a5e79446b65264e8b7479020654adb9d454ee487929a747b091f8503472fa3cebd4735e303e3a50ac967a2477c021759218c5acf6c41666e376adb23368dacedc";
pub(crate) const RESPONSE_CHUNKS_CHACHA: &str =
    "a9ecb0ed84766f4153619d12efb66748999fbb05c2f7f6a16528534029f2c272e6938a53b3663196e5";
