//! The session pipeline (M3 design §7): one immutable `Runtime` per config
//! generation, the `Engine` that dials and relays sessions for the inbound
//! listeners, and the session log.
