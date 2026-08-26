//! End-to-end test: the real server binary plus two real client engines.
//!
//! The unit tests in `conn.rs` drive the handshake over an in-memory pipe. This
//! goes through an actual TCP socket and the actual `synctus-server` executable,
//! so it catches wiring mistakes that a duplex stream cannot: argument parsing,
//! listener setup, and the client's own reconnect loop.

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command as ProcCommand, Stdio};
use std::time::{Duration, Instant};

use synctus_core::client::{Client, ClientHandle, Command, ConnState, Event};
use synctus_core::config::ClientConfig;
use synctus_core::model::{Battery, NudgeKind, Presence, StatusSnapshot};

/// A server child process that is killed when the test ends, pass or fail.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    /// Start the relay on a free port and wait until it accepts connections.
    fn start() -> Self {
        let port = free_port();
        let child = spawn_relay(port);
        wait_until_listening(port);
        Server { child, port }
    }

    fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

/// Ask the OS for a free port, then release it.
///
/// A short race is possible but far less flaky than a hard-coded port on a shared
/// CI runner.
fn free_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").expect("bind probe");
    probe.local_addr().expect("probe addr").port()
}

/// Launch `synctus-server` on `port` with no TLS.
///
/// Payloads are still end-to-end encrypted, which is what these tests verify.
fn spawn_relay(port: u16) -> Child {
    ProcCommand::new(env!("CARGO_BIN_EXE_synctus-server"))
        .env("SYNCTUS_BIND", format!("127.0.0.1:{port}"))
        .env("SYNCTUS_LOG", "synctus_server=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start synctus-server")
}

/// The `synctus` management tool talking to the daemon over the admin socket.
///
/// This is the one integration path that a headless dev box would otherwise never
/// exercise: the TUI and the daemon are separate processes and both could compile
/// while disagreeing about the socket protocol. They must not.
#[cfg(unix)]
#[test]
fn the_management_tool_reads_real_status_over_the_admin_socket() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let port = free_port();
    let socket = std::env::temp_dir().join(format!("synctus-admin-{port}.sock"));

    let mut child = ProcCommand::new(env!("CARGO_BIN_EXE_synctus-server"))
        .env("SYNCTUS_BIND", format!("127.0.0.1:{port}"))
        .env("SYNCTUS_LOG", "synctus_server=warn")
        // A fixed path is required so the test can connect; envs must use the
        // real Windows-style assignment even on Unix for the `--admin-socket`
        // arg, which the daemon accepts directly.
        .arg("--admin-socket")
        .arg(&socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start daemon");

    // Wait for the socket to appear rather than guessing at startup timing.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(socket.exists(), "admin socket never appeared");

    // Connect exactly as `synctus` does: one JSON line in, one out.
    let mut stream = UnixStream::connect(&socket).expect("connect to admin socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(b"{\"cmd\":\"status\"}\n").unwrap();
    stream.flush().unwrap();

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let response: synctus_server::admin::Response =
        serde_json::from_str(&line).expect("daemon answered with JSON");

    match response {
        synctus_server::admin::Response::Status(status) => {
            assert_eq!(status.version, synctus_server::version());
            assert_eq!(status.bind, format!("127.0.0.1:{port}"));
            // Fresh daemon: nothing connected yet.
            assert_eq!(status.devices, 0);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&socket);
}

/// Poll the port until a TCP connection succeeds.
///
/// Probing the socket rather than parsing the log output: it does not depend on
/// log wording, level or the child's stderr buffering, all of which have no
/// business breaking a test.
fn wait_until_listening(port: u16) {
    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(20);

    while Instant::now() < deadline {
        match TcpStream::connect_timeout(
            &addr.parse().expect("valid loopback address"),
            Duration::from_millis(200),
        ) {
            Ok(stream) => {
                // Close immediately; the relay drops a connection that never
                // completes the handshake.
                drop(stream);
                return;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    panic!("relay did not start listening on {addr} within 20s");
}

/// Spawn a client engine on its own runtime, as the real front-ends do.
fn spawn_client(
    addr: &str,
    code: &str,
    device: &str,
) -> (
    ClientHandle,
    tokio::sync::mpsc::Receiver<Event>,
    tokio::runtime::Runtime,
) {
    let cfg = ClientConfig {
        server: addr.to_string(),
        tls: false,
        invite_code: code.to_string(),
        device_id: device.to_string(),
        device_name: format!("device-{device}"),
        ..ClientConfig::default()
    };

    let (handle, events, engine) = Client::spawn(cfg).expect("spawn client");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.spawn(engine.run());
    (handle, events, runtime)
}

/// Block until an event satisfying `predicate` arrives, or fail.
fn wait_for<T>(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    what: &str,
    mut predicate: impl FnMut(&Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if Instant::now() > deadline {
            panic!("timed out waiting for {what}");
        }
        match events.try_recv() {
            Ok(event) => {
                if let Some(value) = predicate(&event) {
                    return value;
                }
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                panic!("client engine exited while waiting for {what}");
            }
        }
    }
}

fn wait_online(events: &mut tokio::sync::mpsc::Receiver<Event>) {
    wait_for(events, "connection", |e| match e {
        Event::State(ConnState::Online) => Some(()),
        Event::State(ConnState::Rejected(why)) => panic!("connection rejected: {why}"),
        _ => None,
    });
}

/// The headline path: two devices with the same code exchange status and pokes.
#[test]
fn two_peers_exchange_status_and_nudges() {
    let server = Server::start();
    let code = "TEST-CODE-ABCD-1234";

    let (alice, mut alice_events, _rt_a) = spawn_client(&server.addr(), code, "alice");
    let (bob, mut bob_events, _rt_b) = spawn_client(&server.addr(), code, "bob");

    wait_online(&mut alice_events);
    wait_online(&mut bob_events);

    // Alice publishes a status; Bob must receive the decrypted snapshot.
    let mut snapshot = StatusSnapshot::new("alice", "Alice PC");
    snapshot.presence = Presence::Resting;
    snapshot.battery = Some(Battery {
        percent: 73,
        charging: true,
        minutes_left: None,
    });
    alice.publish(snapshot).expect("publish");

    let received = wait_for(&mut bob_events, "Alice's status", |e| match e {
        Event::PeerStatus(s) if s.device_id == "alice" => Some(s.clone()),
        _ => None,
    });
    assert_eq!(received.name, "Alice PC");
    assert_eq!(received.presence, Presence::Resting);
    assert_eq!(received.battery.map(|b| b.percent), Some(73));

    // Bob pokes Alice.
    bob.nudge(synctus_core::model::Nudge::new(NudgeKind::Knock, "Bob"))
        .expect("nudge");

    let nudge = wait_for(&mut alice_events, "Bob's nudge", |e| match e {
        Event::Nudge(n) => Some(n.clone()),
        _ => None,
    });
    assert_eq!(nudge.kind, NudgeKind::Knock);
    assert_eq!(nudge.from_name, "Bob");

    let _ = alice.send(Command::Shutdown);
    let _ = bob.send(Command::Shutdown);
}

/// A device that connects later must immediately receive the retained status.
#[test]
fn late_joiner_receives_retained_status() {
    let server = Server::start();
    let code = "RETAIN-CODE-9876";

    let (alice, mut alice_events, _rt_a) = spawn_client(&server.addr(), code, "alice");
    wait_online(&mut alice_events);

    let mut snapshot = StatusSnapshot::new("alice", "Alice PC");
    snapshot.presence = Presence::Busy;
    alice.publish(snapshot).expect("publish");

    // Give the relay a moment to record the retained frame.
    std::thread::sleep(Duration::from_millis(300));

    // Bob starts only now and must still see Alice without waiting for her next
    // update.
    let (bob, mut bob_events, _rt_b) = spawn_client(&server.addr(), code, "bob");
    wait_online(&mut bob_events);

    let received = wait_for(&mut bob_events, "retained status", |e| match e {
        Event::PeerStatus(s) if s.device_id == "alice" => Some(s.clone()),
        _ => None,
    });
    assert_eq!(received.presence, Presence::Busy);

    let _ = alice.send(Command::Shutdown);
    let _ = bob.send(Command::Shutdown);
}

/// Two different invite codes must be completely isolated, even on the same
/// relay.
///
/// This is the property that matters in practice: a wrong code does not just fail
/// to authenticate, it lands in a different room and therefore cannot observe
/// anything. The "occupied room rejects a mismatched MAC" path is covered by the
/// unit tests in `conn.rs`, which can forge a room id directly.
#[test]
fn different_invite_codes_are_isolated() {
    let server = Server::start();

    // Alice occupies her room.
    let (alice, mut alice_events, _rt_a) =
        spawn_client(&server.addr(), "ROOM-CODE-AAAA-1111", "alice");
    wait_online(&mut alice_events);

    // Mallory has a different code, so HKDF puts her in a different room id.
    let (mallory, mut mallory_events, _rt_m) =
        spawn_client(&server.addr(), "ROOM-CODE-AAAA-2222", "mallory");
    wait_online(&mut mallory_events);

    let mut snapshot = StatusSnapshot::new("alice", "Alice PC");
    snapshot.presence = Presence::Active;
    alice.publish(snapshot).expect("publish");

    // Mallory is in a different room, so nothing arrives.
    std::thread::sleep(Duration::from_secs(1));
    let mut saw_status = false;
    while let Ok(event) = mallory_events.try_recv() {
        if matches!(event, Event::PeerStatus(_)) {
            saw_status = true;
        }
    }
    assert!(
        !saw_status,
        "a different invite code must not receive the peer's status"
    );

    let _ = alice.send(Command::Shutdown);
    let _ = mallory.send(Command::Shutdown);
}

/// The client must recover on its own when the relay goes away and comes back.
#[test]
fn client_reconnects_after_the_server_restarts() {
    let server = Server::start();
    let addr = server.addr();
    let code = "RECONNECT-CODE-42";

    let (alice, mut alice_events, _rt_a) = spawn_client(&addr, code, "alice");
    wait_online(&mut alice_events);

    // Kill the relay; the client should report the drop.
    drop(server);
    wait_for(&mut alice_events, "disconnection", |e| match e {
        Event::State(ConnState::Offline(_)) => Some(()),
        _ => None,
    });

    // Bring a relay back on the same port. The backoff starts at one second, so
    // the client should be back well inside the wait_for timeout.
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    let mut child = spawn_relay(port);
    wait_until_listening(port);

    wait_online(&mut alice_events);

    let _ = alice.send(Command::Shutdown);
    let _ = child.kill();
    let _ = child.wait();
}
