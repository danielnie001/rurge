//! Vectors for the keyed primitives of Shadow TLS. They were computed with
//! Python's `hmac` / `hashlib` by a script that mirrors, step by step, what
//! the reference implementation does (`ihciah/shadow-tls`: `Hmac`, `kdf`,
//! `xor_slice`, `generate_session_id`, `copy_by_frame_with_modification`,
//! `copy_add_appdata`, `verify_appdata`) and what `sing-shadowtls` does
//! (`v2_hash.go`, `v3_client.go`, `v3_server.go`, `v3_conn.go`). The fake
//! server shares these primitives with the client, so without the vectors a
//! mistake made on both sides would go unnoticed.
//!
//! Inputs: the password `vector-password`; the ServerRandom `20 21 .. 3f`.

use super::auth::{Chain, V2_TAG, xor_key};

const PASSWORD: &[u8] = b"vector-password";

fn unhex(parts: &[&str]) -> Vec<u8> {
    let text = parts.concat();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn server_random() -> [u8; 32] {
    std::array::from_fn(|i| 0x20 + i as u8)
}

#[test]
fn the_xor_key_is_sha256_of_password_and_server_random() {
    assert_eq!(
        xor_key(PASSWORD, &server_random())[..],
        unhex(&["e2eccdfee33a7db066a43dd90958affcb12c1cc10bfe4b6e0eec29e9ab936ca9"])[..]
    );
}

#[test]
fn data_tags_chain_per_direction_and_feed_themselves_back() {
    for (side, tags) in [
        (&b"C"[..], ["23155410", "352ad183", "2ab277ef"]),
        (&b"S"[..], ["aac92819", "6b406818", "571bab9d"]),
    ] {
        let mut chain = Chain::new(PASSWORD, &[&server_random(), side]);
        let third = b"third ".repeat(50);
        for (payload, tag) in [&b"first payload"[..], &b""[..], &third[..]]
            .into_iter()
            .zip(tags)
        {
            assert_eq!(chain.frame_tag(payload)[..], unhex(&[tag])[..]);
        }
    }
}

#[test]
fn the_v2_digest_is_eight_bytes_over_everything_fed() {
    let mut digest = Chain::new(PASSWORD, &[]);
    for part in [&b"ServerHello..."[..], b"ChangeCipherSpec", b"...Finished"] {
        digest.update(part);
    }
    assert_eq!(
        digest.digest::<V2_TAG>()[..],
        unhex(&["6ded756644915408"])[..]
    );
}
