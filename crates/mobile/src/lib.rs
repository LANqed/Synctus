//! Synctus Android native library.
//!
//! Two layers:
//!
//! * [`bridge`] — the engine plus a JSON command/event protocol. No JNI, no
//!   Android APIs, so it compiles and unit-tests on the host.
//! * [`jni_api`] — thin JNI exports that marshal strings in and out. Android
//!   only.

pub mod bridge;

#[cfg(target_os = "android")]
mod jni_api;

pub use bridge::{
    default_config_json, global_command, global_local_status, global_poll, global_running,
    global_start, global_stop, new_invite_code, Bridge, BridgeCommand, BridgeEvent,
};
