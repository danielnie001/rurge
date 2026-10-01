//! `snell` outbound, versions 4 and 5 (manual: Policies › Snell; phase 2 M6
//! design 4): the record stream (`record`) keyed per direction by Argon2id
//! of the PSK (`kdf`).

pub(crate) mod kdf;
pub(crate) mod record;
