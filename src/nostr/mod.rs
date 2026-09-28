//! The nostr layer's shared code: key handling, the vault, the socket
//! protocol the daemon serves, and the bunker's NIP-46 brain.

pub mod bunker;
pub mod keys;
pub mod protocol;
pub mod socket;
pub mod vault;
