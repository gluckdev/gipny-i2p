#![cfg(feature = "web-gateway")]

use std::{sync::Arc, time::Duration};
use futures_util::{SinkExt, StreamExt};
use gipny_lib::{data_lock::DataDirLock, web_server::{self, LocalPolicy, WebServerState}, AppCtx};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}};
use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, http::{header::ORIGIN, StatusCode}, Error, Message}};

const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
struct Server { port: u16, task: tokio::task::JoinHandle<std::io::Result<()>>, _dir: tempfile::TempDir }
impl Drop for Server { fn drop(&mut self) { self.task.abort(); } }
impl Server {
    async fn start() -> Self { Self::start_on(0).await }
    async fn start_on(requested_port: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<!doctype html><title>local test</title>").unwrap();
        let listener = TcpListener::bind(web_server::resolve_listen_addr(None, Some(requested_port)).unwrap()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let state = Arc::new(WebServerState { ctx: Arc::new(AppCtx::new(dir.path().into())), event_tx });
        let router = web_server::build_router(state, dir.path().into(), Some(LocalPolicy {
            port, token: TOKEN.into(), allowed_origins: vec!["https://custom.example".into()],
        }));
        Self { port, task: tokio::spawn(web_server::serve(listener, router)), _dir: dir }
    }
    async fn get(&self, path: &str, host: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        String::from_utf8(bytes).unwrap()
    }
    async fn ws(&self, token: Option<&str>, origin: Option<&str>) -> Result<(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, tokio_tungstenite::tungstenite::handshake::client::Response), Error> {
        let suffix = token.map(|s| format!("?token={s}")).unwrap_or_default();
        let mut request = format!("ws://127.0.0.1:{}/ws{suffix}", self.port).into_client_request().unwrap();
        if let Some(origin) = origin { request.headers_mut().insert(ORIGIN, origin.parse().unwrap()); }
        connect_async(request).await
    }
}

#[tokio::test]
async fn local_http_and_websocket_boundary() {
    let mut server = Server::start().await;
    let host = format!("127.0.0.1:{}", server.port);
    for path in ["/", "/health", "/missing", "/ws"] {
        let response = server.get(path, "evil.example").await.to_lowercase();
        assert!(response.starts_with("http/1.1 403"), "{response}");
        assert!(response.contains("cache-control: no-store"));
    }
    let html = server.get("/", &host).await.to_lowercase();
    assert!(html.starts_with("http/1.1 200"), "{html}");
    for header in ["cache-control: no-store", "content-security-policy:", "x-content-type-options: nosniff", "referrer-policy: no-referrer"] { assert!(html.contains(header), "{html}"); }
    assert!(html.contains(&format!("ws://127.0.0.1:{}", server.port)));
    assert!(!html.contains("access-control-allow-origin"));
    assert!(server.get("/health", &format!("localhost:{}", server.port)).await.starts_with("HTTP/1.1 200"));
    for (token, origin, status) in [
        (None, Some("http://127.0.0.1"), StatusCode::UNAUTHORIZED),
        (Some("wrong"), Some(host.as_str()), StatusCode::UNAUTHORIZED),
        (Some("BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"), Some(host.as_str()), StatusCode::UNAUTHORIZED),
        (Some(TOKEN), Some("https://evil.example"), StatusCode::FORBIDDEN),
        (Some(TOKEN), None, StatusCode::FORBIDDEN),
    ] {
        match server.ws(token, origin).await {
            Err(Error::Http(response)) => assert_eq!(response.status(), status),
            other => panic!("expected {status}, received {other:?}"),
        }
    }
    for origin in [format!("http://{host}"), format!("http://localhost:{}", server.port), "https://gluckdev.github.io".into(), "https://custom.example".into()] {
        let (mut socket, response) = server.ws(Some(TOKEN), Some(&origin)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        socket.send(Message::Text(r#"{"id":1,"method":"web_info"}"#.into())).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(3), socket.next()).await.unwrap().unwrap().unwrap();
        let reply: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["mode"], "local");
        assert_eq!(reply["result"]["version"], env!("CARGO_PKG_VERSION"));
        socket.close(None).await.unwrap();
    }
    // Reuse the now-known port through the same address resolver as --port N.
    let pinned_port = server.port;
    server.task.abort();
    let _ = (&mut server.task).await;
    drop(server);
    let pinned = Server::start_on(pinned_port).await;
    assert_eq!(pinned.port, pinned_port);
    assert!(pinned.get("/health", "evil.example").await.starts_with("HTTP/1.1 403"));
    match pinned.ws(None, Some(&format!("http://127.0.0.1:{pinned_port}"))).await {
        Err(Error::Http(response)) => assert_eq!(response.status(), StatusCode::UNAUTHORIZED),
        other => panic!("fixed-port local policy must require a token: {other:?}"),
    }
    let (mut socket, response) = pinned.ws(Some(TOKEN), Some("https://gluckdev.github.io")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    socket.close(None).await.unwrap();

}

#[test]
fn data_directory_excludes_a_second_owner_and_releases_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("instance.lock"), "legacy sentinel PID\n").unwrap();
    let guard = DataDirLock::acquire(dir.path()).unwrap();
    assert!(std::fs::read(dir.path().join("instance.lock")).unwrap().is_empty());
    let path = dir.path().join("instance.lock");
    let other = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    assert_eq!(DataDirLock::acquire(dir.path()).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    drop(guard);
    other.try_lock().unwrap();
    drop(other);
    assert!(path.exists(), "the reusable lock file must not be unlinked");
    assert!(DataDirLock::acquire(dir.path()).is_ok());
}

#[test]
fn launch_capabilities_are_fresh_and_url_safe() {
    let first = gipny_libcore::security::web_launch_token();
    let second = gipny_libcore::security::web_launch_token();
    assert_ne!(first, second);
    assert_eq!(first.len(), 43);
    assert!(first.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
}


#[test]
fn pinned_ports_and_explicit_loopback_keep_local_mode() {
    for listen in [None, Some("127.0.0.1:4567".parse().unwrap()), Some("[::1]:4567".parse().unwrap())] {
        let addr = web_server::resolve_listen_addr(listen, listen.is_none().then_some(4567)).unwrap();
        assert_eq!(addr, "127.0.0.1:4567".parse().unwrap());
    }
    let legacy = "0.0.0.0:8080".parse().unwrap();
    assert_eq!(web_server::resolve_listen_addr(Some(legacy), None).unwrap(), legacy);
    assert!(web_server::resolve_listen_addr(Some(legacy), Some(8080)).is_err());
}

#[test]
fn additional_origins_are_normalized_and_reject_paths() {
    assert_eq!(web_server::normalize_origin("HTTPS://EXAMPLE.ORG:8443").unwrap(), "https://example.org:8443");
    for value in ["https://example.org/", "https://example.org/path", "example.org", "https:///", "https://example.org?q=1", "https://example.org#x", "https://user@example.org"] {
        assert!(web_server::normalize_origin(value).is_err(), "{value}");
    }
}
