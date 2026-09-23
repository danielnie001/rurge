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

use super::ServerSide;
use super::auth::{Chain, V2_TAG, xor_key};
use super::sign::hello_tag;

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

const HELLO: &[&str] = &[
    "160301006f0100006b0303000102030405060708090a0b0c0d0e0f1011121314",
    "15161718191a1b1c1d1e1f20808182838485868788898a8b8c8d8e8f90919293",
    "9495969798999a9b9c9d9e9fc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3",
    "d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7",
];

#[test]
fn the_client_hello_tag_covers_the_hello_without_its_record_header() {
    // HMAC-SHA1(password, hello[5..72] || 00 00 00 00 || hello[76..]), 4 bytes
    assert_eq!(hello_tag(PASSWORD, &unhex(HELLO)), unhex(&["a7640deb"])[..]);
}

const HANDSHAKE_PLAIN_0: &[&str] = &[
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
    "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
    "60616263",
];
const HANDSHAKE_WIRE_0: &[&str] = &[
    "1703030068312db371e2edcffde73f7bb76ead37d20555a1f3a13d0ed21feb5d",
    "7916f533f2b78e72b6c2cdefddc71f5b974e8d17f2257581d3811d2ef23fcb7d",
    "5936d513d297ae5296a2ad8fbda77f3bf72eed77924515e1b3e17d4e925fab1d",
    "3956b573b2f7ce32f6828daf9d",
];
const HANDSHAKE_PLAIN_1: &[&str] = &[
    "c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7",
    "e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9c8c9cacbcccdcecfd0d1d2d3d4d5",
    "d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5",
    "f6f7f8f9",
];
const HANDSHAKE_WIRE_1: &[&str] = &[
    "1703030068e517c5622a2507352ff7b37fb675ef0add8d792b69f5c61ad72395",
    "b1ee0dcb0a4f768a4e0a0527150fd7935f9655cf2afdad590b49d5d408c13587",
    "a3c023f9387940b87c343b152739e1a16db87bdd38ebbb4b1957cbf428e115a7",
    "83e003d9185960985c141b3507",
];

#[test]
fn handshake_records_verify_in_order_and_come_back_as_the_site_wrote_them() {
    let mut side = ServerSide::new(PASSWORD, &server_random());
    for (wire, plain) in [
        (HANDSHAKE_WIRE_0, HANDSHAKE_PLAIN_0),
        (HANDSHAKE_WIRE_1, HANDSHAKE_PLAIN_1),
    ] {
        let mut record = unhex(wire);
        assert!(side.restore(&mut record));
        let plain = unhex(plain);
        assert_eq!(record[..3], [23, 3, 3]);
        assert_eq!(
            usize::from(u16::from_be_bytes([record[3], record[4]])),
            plain.len()
        );
        assert_eq!(record[5..], plain[..]);
    }
    // the chain ran on: the first record does not verify a second time
    let mut again = unhex(HANDSHAKE_WIRE_0);
    let before = again.clone();
    assert!(!side.restore(&mut again));
    assert_eq!(
        again, before,
        "a record that is not ours is left as it came"
    );
}

#[test]
fn a_flipped_bit_anywhere_in_a_handshake_record_is_noticed() {
    for at in [5usize, 8, 9, 60, 108] {
        let mut side = ServerSide::new(PASSWORD, &server_random());
        let mut record = unhex(HANDSHAKE_WIRE_0);
        record[at] ^= 0x10;
        assert!(!side.restore(&mut record), "byte {at}");
    }
}
