//! Synctus relay server.
//!
//! Split into a library so the daemon (`synctus-server`) and the management tool
//! (`synctusctl`) share one definition of the config file and the admin socket
//! protocol. Two copies of a struct that has to agree is how a config option ends
//! up silently ignored by one of them.

pub mod admin;
pub mod config;
pub mod conn;
pub mod hub;
pub mod limiter;
pub mod service;

/// Version of the running build, used by both binaries and reported over the
/// admin socket.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
