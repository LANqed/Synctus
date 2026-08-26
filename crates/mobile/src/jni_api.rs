//! JNI exports consumed by `dev.synctus.app.NativeBridge`.
//!
//! Every function follows the same shape: convert the Java strings to Rust,
//! delegate to [`crate::bridge`], and convert the result back. Errors are
//! returned as strings rather than thrown exceptions — the Kotlin side treats a
//! non-empty return as a failure message it can show in the UI, which is simpler
//! than exception plumbing for a four-function surface.
//!
//! Panics are contained with `catch_unwind`: unwinding across the FFI boundary is
//! undefined behaviour, and a panic here would take the whole app down.

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

/// Read a Java string. Returns `None` if the JVM handed us something unusable.
fn read_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value).ok().map(|s| s.into())
}

/// Build a Java string, falling back to a null pointer the Kotlin side reads as
/// `null`.
fn make_string(env: &mut JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(s) => s.into_raw(),
        Err(e) => {
            tracing::error!(error = %e, "无法创建 Java 字符串");
            std::ptr::null_mut()
        }
    }
}

/// Run `f`, turning a panic into an error message instead of unwinding into Java.
fn guard<F: FnOnce() -> String + std::panic::UnwindSafe>(f: F) -> String {
    match std::panic::catch_unwind(f) {
        Ok(value) => value,
        Err(_) => "内部错误：原生层发生 panic".to_string(),
    }
}

/// Initialise logging. Safe to call more than once.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeInit(_env: JNIEnv, _class: JClass) {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("Synctus"),
        );
        // Route `tracing` events from the core crates into logcat.
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();
        tracing::info!("Synctus 原生层已初始化 v{}", env!("CARGO_PKG_VERSION"));
    });
}

/// Start the engine. Returns an empty string on success, else the error.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeStart(
    mut env: JNIEnv,
    _class: JClass,
    config_json: JString,
) -> jstring {
    let config = read_string(&mut env, &config_json);
    let message = guard(move || match config {
        Some(json) => match crate::bridge::global_start(&json) {
            Ok(()) => String::new(),
            Err(e) => format!("{e:#}"),
        },
        None => "配置字符串无效".to_string(),
    });
    make_string(&mut env, &message)
}

/// Send a command. Returns an empty string on success, else the error.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeCommand(
    mut env: JNIEnv,
    _class: JClass,
    command_json: JString,
) -> jstring {
    let command = read_string(&mut env, &command_json);
    let message = guard(move || match command {
        Some(json) => match crate::bridge::global_command(&json) {
            Ok(()) => String::new(),
            Err(e) => format!("{e:#}"),
        },
        None => "命令字符串无效".to_string(),
    });
    make_string(&mut env, &message)
}

/// Poll pending events as a JSON array.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativePoll(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let json = guard(crate::bridge::global_poll);
    make_string(&mut env, &json)
}

/// Local status as a JSON object, for the foreground notification.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeLocalStatus(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let json = guard(crate::bridge::global_local_status);
    make_string(&mut env, &json)
}

#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeStop(_env: JNIEnv, _class: JClass) {
    let _ = guard(|| {
        crate::bridge::global_stop();
        String::new()
    });
}

#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeRunning(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    let running = std::panic::catch_unwind(crate::bridge::global_running).unwrap_or(false);
    if running {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

/// Fresh pairing code, so the Android UI does not reimplement the alphabet.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeNewInviteCode(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let code = guard(crate::bridge::new_invite_code);
    make_string(&mut env, &code)
}

/// Default configuration as JSON, used to seed the settings screen.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeDefaultConfig(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let json = guard(crate::bridge::default_config_json);
    make_string(&mut env, &json)
}

/// Native library version, shown in the about screen and used by the update
/// check.
#[no_mangle]
pub extern "system" fn Java_dev_synctus_app_NativeBridge_nativeVersion(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    make_string(&mut env, env!("CARGO_PKG_VERSION"))
}
