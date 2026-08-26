//! The WebUI: a small, read-mostly admin dashboard for the relay.
//!
//! Scope is deliberate. The relay's admin socket is Unix-only and guarded by file
//! permissions; a web server is reachable over the network, so it needs a
//! password instead. And it is *read-mostly*: the only mutating action is
//! disconnecting a device, which the relay can do itself. Starting, stopping and
//! restarting the daemon belong to the service manager (`synctus` /
//! `systemctl`), not to a web page — a browser is a worse place than a terminal
//! for lifecycle control, and it would need credentials the relay should not
//! have.
//!
//! Auth is HTTP Basic, checked against the password from the config file. There
//! is deliberately no user model: this is a two-person relay and one admin
//! password is the right amount of ceremony.
//!
//! The server runs on a dedicated thread (`tiny_http` is blocking) and answers
//! queries by blocking on the shared tokio runtime, which is fine because every
//! query is a short hub lookup.

use anyhow::{anyhow, Context, Result};
use std::sync::Arc;

use crate::hub::Hub;

/// Start the web server on a dedicated thread.
///
/// Failure to bind is fatal for the feature but not the relay: a port conflict
/// should not take the relay down.
pub fn spawn(bind: &str, password: &str, hub: Arc<Hub>) -> Result<()> {
    let server =
        tiny_http::Server::http(bind).map_err(|e| anyhow!("WebUI 监听失败 {bind}: {e}"))?;
    let handle = tokio::runtime::Handle::current();
    let password = password.to_string();

    tracing::info!(bind = %bind, "WebUI 已启动");

    std::thread::Builder::new()
        .name("synctus-web".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                // One thread per request keeps a slow client from stalling the
                // rest of the page; admin traffic is tiny so this is cheap.
                let handle = handle.clone();
                let hub = hub.clone();
                let password = password.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_request(request, hub, &handle, &password) {
                        tracing::debug!(error = %format!("{e:#}"), "WebUI 请求处理失败");
                    }
                });
            }
        })
        .context("启动 WebUI 线程失败")?;

    Ok(())
}

/// A request with the password already checked; failures here become 500s.
fn handle_request(
    mut request: tiny_http::Request,
    hub: Arc<Hub>,
    handle: &tokio::runtime::Handle,
    password: &str,
) -> Result<()> {
    let url = request.url();
    let method = request.method();

    // Everything is behind the password, including the page itself: the browser
    // prompts for credentials when it first sees a 401.
    if !authorized(&request, password) {
        return respond_unauthorized(request, "需要管理员密码\n".to_string());
    }

    let (status, content_type, body) = match (method, url) {
        (&tiny_http::Method::Get, "/") => (200, "text/html; charset=utf-8", index_html()),
        (&tiny_http::Method::Get, "/api/status") => {
            let snapshot = handle.block_on(hub.snapshot());
            let json = String::from_utf8(serde_json::to_vec(&snapshot)?)?;
            (200, "application/json", json)
        }
        (&tiny_http::Method::Post, "/api/kick") => {
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body)?;

            #[derive(serde::Deserialize)]
            struct Kick {
                device: String,
            }
            let device = serde_json::from_str::<Kick>(&body)
                .map(|k| k.device)
                .unwrap_or_default();

            // `hub.kick` is async and this thread is outside the runtime; block
            // on the shared handle, which is fine for a short hub lookup.
            let kicked = !device.is_empty() && handle.block_on(hub.kick(&device));
            let json =
                String::from_utf8(serde_json::to_vec(&serde_json::json!({ "ok": kicked }))?)?;
            (200, "application/json", json)
        }
        _ => (404, "text/plain; charset=utf-8", "not found\n".to_string()),
    };

    respond(request, status, content_type, body)
}

fn authorized(request: &tiny_http::Request, password: &str) -> bool {
    use base64::Engine;

    let header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str())
        .unwrap_or("");

    // "Basic base64(admin:password)". The username is ignored: there is one admin
    // password and no user model.
    let Some(encoded) = header.strip_prefix("Basic ") else {
        return false;
    };

    let decoded = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(d) => String::from_utf8_lossy(&d).into_owned(),
        Err(_) => return false,
    };

    let Some((_, provided)) = decoded.rsplit_once(':') else {
        return false;
    };

    constant_time_eq(provided.as_bytes(), password.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn header(name: &str, value: &str) -> Result<tiny_http::Header> {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .map_err(|_| anyhow!("构造 HTTP 头失败: {name}"))
}

fn respond(
    request: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: String,
) -> Result<()> {
    let response = tiny_http::Response::new(
        tiny_http::StatusCode(status),
        vec![
            header("Content-Type", content_type)?,
            header("Cache-Control", "no-store")?,
        ],
        std::io::Cursor::new(body),
        None,
        None,
    );
    request.respond(response).context("发送响应失败")
}

/// 401 with the `WWW-Authenticate` header, which makes the browser show its own
/// username/password prompt.
fn respond_unauthorized(request: tiny_http::Request, body: String) -> Result<()> {
    let mut response = tiny_http::Response::new(
        tiny_http::StatusCode(401),
        vec![
            header("Content-Type", "text/plain; charset=utf-8")?,
            header("Cache-Control", "no-store")?,
        ],
        std::io::Cursor::new(body),
        None,
        None,
    );
    response.add_header(header("WWW-Authenticate", "Basic realm=\"synctus\"")?);
    request.respond(response).context("发送响应失败")
}

/// The single page. A self-contained HTML file with inline CSS and JS: the
/// server serves one route and the browser does the rest. No build step, no
/// assets to serve.
fn index_html() -> String {
    include_str!("web_index.html").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_with_auth(header_value: &str) -> tiny_http::Request {
        let header = tiny_http::Header::from_bytes(b"Authorization", header_value.as_bytes())
            .expect("ascii header");
        tiny_http::TestRequest::new().with_header(header).into()
    }

    #[test]
    fn correct_password_passes() {
        // "admin:secret" base64.
        let req = req_with_auth("Basic YWRtaW46c2VjcmV0");
        assert!(authorized(&req, "secret"));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let req = req_with_auth("Basic YWRtaW46d3Jvbmc=");
        assert!(!authorized(&req, "secret"));
    }

    #[test]
    fn missing_header_is_rejected() {
        let req: tiny_http::Request = tiny_http::TestRequest::new().into();
        assert!(!authorized(&req, "secret"));
    }

    #[test]
    fn malformed_basic_is_rejected() {
        let req = req_with_auth("Basic !!!not-base64!!!");
        assert!(!authorized(&req, "secret"));
    }

    #[test]
    fn comparison_is_constant_time_in_shape() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn the_page_is_served() {
        let html = index_html();
        assert!(html.contains("api/status"), "page must poll the status API");
    }
}
