//! HTTPS GET to the clearnet through i2p, without a local HTTP proxy.
//!
//! A stream from our own destination to the i2p outproxy, a `CONNECT` through
//! it, TLS to the host inside that, HTTP/1.1 on top. Everything is inside this
//! process: nothing listens on any port. The outproxy sees which host is
//! asked for (never the content: TLS ends at the host) and never who asks.
//!
//! Only as much HTTP as the updater needs: GET, redirects, a streamed body.

use std::sync::Arc;

use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net::TorNode;

/// `exit.stormycloud.i2p`, the outproxy i2p ships as its default. There is no
/// address book in the in-process router, so its b32 address is written out;
/// from StormyCloud's own page (https://www.stormycloud.org/updating-i2p-outproxy/).
pub const OUTPROXY: &str = "5d4s7pcvfdpftfk7npc7hllyujhufsdprtrf4o53i44rgsa2xbwa.b32.i2p";

const MAX_REDIRECTS: usize = 5;
const MAX_CONNECT_REPLY: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("bad url: {0}")]
    Url(String),
    #[error("outproxy: {0}")]
    Outproxy(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("http: {0}")]
    Http(String),
    #[error("status {0}")]
    Status(u16),
    #[error("too many redirects")]
    Redirects,
}

/// A response body, read chunk by chunk.
pub struct Body {
    inner: hyper::body::Incoming,
}

impl Body {
    pub async fn chunk(&mut self) -> Result<Option<bytes::Bytes>, HttpError> {
        loop {
            match self.inner.frame().await {
                None => return Ok(None),
                Some(Err(e)) => return Err(HttpError::Http(e.to_string())),
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        return Ok(Some(data));
                    }
                }
            }
        }
    }

    pub async fn text(mut self) -> Result<String, HttpError> {
        let mut out = Vec::new();
        while let Some(c) = self.chunk().await? {
            out.extend_from_slice(&c);
        }
        String::from_utf8(out).map_err(|e| HttpError::Http(e.to_string()))
    }
}

/// GET `url` (https only), following redirects. Non-2xx is an error.
pub async fn get(node: &Arc<TorNode>, tls: &Arc<rustls::ClientConfig>, url: &str, accept: Option<&str>) -> Result<Body, HttpError> {
    let mut url = url::Url::parse(url).map_err(|e| HttpError::Url(e.to_string()))?;
    for _ in 0..=MAX_REDIRECTS {
        let resp = get_once(node, tls, &url, accept).await?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp.headers().get(hyper::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| HttpError::Http("redirect without Location".into()))?;
            url = url.join(loc).map_err(|e| HttpError::Url(e.to_string()))?;
            continue;
        }
        if !status.is_success() {
            return Err(HttpError::Status(status.as_u16()));
        }
        return Ok(Body { inner: resp.into_body() });
    }
    Err(HttpError::Redirects)
}

async fn get_once(
    node: &Arc<TorNode>,
    tls: &Arc<rustls::ClientConfig>,
    url: &url::Url,
    accept: Option<&str>,
) -> Result<hyper::Response<hyper::body::Incoming>, HttpError> {
    if url.scheme() != "https" {
        return Err(HttpError::Url(format!("only https, not {}", url.scheme())));
    }
    let host = url.host_str().ok_or_else(|| HttpError::Url("no host".into()))?.to_string();
    let port = url.port_or_known_default().unwrap_or(443);

    // To the outproxy over i2p, and through it to the host.
    let mut stream = node.connect_service(OUTPROXY, 80).await
        .map_err(|e| HttpError::Outproxy(format!("{e:?}")))?
        .into_inner();
    let connect = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
    stream.write_all(connect.as_bytes()).await.map_err(|e| HttpError::Outproxy(e.to_string()))?;
    stream.flush().await.map_err(|e| HttpError::Outproxy(e.to_string()))?;
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    while !reply.ends_with(b"\r\n\r\n") {
        if reply.len() > MAX_CONNECT_REPLY {
            return Err(HttpError::Outproxy("reply to CONNECT too long".into()));
        }
        let n = stream.read(&mut byte).await.map_err(|e| HttpError::Outproxy(e.to_string()))?;
        if n == 0 {
            return Err(HttpError::Outproxy("closed during CONNECT".into()));
        }
        reply.push(byte[0]);
    }
    let status_line = String::from_utf8_lossy(&reply);
    let status_line = status_line.lines().next().unwrap_or_default();
    if status_line.split_whitespace().nth(1) != Some("200") {
        return Err(HttpError::Outproxy(format!("CONNECT refused: {status_line}")));
    }

    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| HttpError::Tls(e.to_string()))?;
    let tls_stream = tokio_rustls::TlsConnector::from(tls.clone()).connect(name, stream).await
        .map_err(|e| HttpError::Tls(e.to_string()))?;

    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls_stream)).await
        .map_err(|e| HttpError::Http(e.to_string()))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let path = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let mut req = hyper::Request::get(path)
        .header(hyper::header::HOST, host)
        .header(hyper::header::USER_AGENT, "gipny-i2p-updater");
    if let Some(a) = accept {
        req = req.header(hyper::header::ACCEPT, a);
    }
    let req = req.body(http_body_util::Empty::<bytes::Bytes>::new()).map_err(|e| HttpError::Http(e.to_string()))?;
    sender.send_request(req).await.map_err(|e| HttpError::Http(e.to_string()))
}
