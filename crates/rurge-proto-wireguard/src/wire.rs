//! What rurge reads or writes in a WireGuard message itself: the message
//! type (byte 0) and the three reserved bytes after it, which `client-id`
//! fills (manual: WARP routes by them).

/// A handshake initiation (the WireGuard paper, §5.4.2).
pub const HANDSHAKE_INITIATION: u8 = 1;
/// A handshake response (§5.4.3).
pub const HANDSHAKE_RESPONSE: u8 = 2;
/// The TOS byte a handshake initiation is sent with: DSCP AF41, as the
/// manual and WireGuard itself mark it.
pub const HANDSHAKE_TOS: u8 = 0x88;

/// The type of `message`.
pub fn message_type(message: &[u8]) -> Option<u8> {
    message.first().copied()
}

/// Writes `client_id` into the reserved bytes of an outgoing message.
pub fn mark(message: &mut [u8], client_id: Option<[u8; 3]>) {
    if let (Some(id), Some(reserved)) = (client_id, message.get_mut(1..4)) {
        reserved.copy_from_slice(&id);
    }
}

/// Clears the reserved bytes of an incoming message before boringtun reads
/// it (it takes them for part of the type): a server that routes by
/// `client-id` sends them back filled in. Standard peers send zeros.
pub fn unmark(message: &mut [u8]) {
    if let Some(reserved) = message.get_mut(1..4) {
        reserved.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_bytes_carry_the_client_id_out_and_are_cleared_in() {
        let mut message = [HANDSHAKE_INITIATION, 0, 0, 0, 0xaa, 0xbb];
        mark(&mut message, Some([83, 12, 235]));
        assert_eq!(message, [1, 83, 12, 235, 0xaa, 0xbb]);
        unmark(&mut message);
        assert_eq!(message, [1, 0, 0, 0, 0xaa, 0xbb]);
        assert_eq!(message_type(&message), Some(HANDSHAKE_INITIATION));
        // without an id nothing is written; what is too short is left alone
        mark(&mut message, None);
        assert_eq!(message, [1, 0, 0, 0, 0xaa, 0xbb]);
        let mut short = [4u8, 1];
        mark(&mut short, Some([9, 9, 9]));
        unmark(&mut short);
        assert_eq!(short, [4, 1]);
        assert_eq!(message_type(&[]), None);
    }
}
