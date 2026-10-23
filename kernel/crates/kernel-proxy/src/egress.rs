//! The kernel's client side of the Root egress socket (kernel/proxy.md §3.7; root/egress.md §4.1):
//! plain HTTP/1.1 over a Unix socket, a small keep-alive pool, no TLS.

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::UnixStream;

pub type Sender = http1::SendRequest<Full<Bytes>>;

/// Why a request did not get a response head.
#[derive(Debug)]
pub enum EgressError {
    /// The socket could not be reached (missing, refused, connect timeout): the request never left
    /// the agent zone, so it is safe to retry.
    Connect(String),
    /// The connection broke after the request was handed over: it may have been billed.
    Transport(String),
    /// No response head within the header timeout.
    Timeout,
}

/// The egress client and its pool.
pub struct Egress {
    path: PathBuf,
    pool: Mutex<Vec<Sender>>,
    max_idle: usize,
    connect_timeout: Duration,
    pub connects: AtomicU64,
    pub requests: AtomicU64,
}

impl Egress {
    pub fn new(path: &Path, max_idle: usize, connect_timeout: Duration) -> Egress {
        Egress {
            path: path.to_path_buf(),
            pool: Mutex::new(Vec::new()),
            max_idle,
            connect_timeout,
            connects: AtomicU64::new(0),
            requests: AtomicU64::new(0),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn connect(&self) -> Result<Sender, EgressError> {
        let stream = tokio::time::timeout(self.connect_timeout, UnixStream::connect(&self.path))
            .await
            .map_err(|_| EgressError::Connect("connect timeout".into()))?
            .map_err(|e| EgressError::Connect(e.to_string()))?;
        let (sender, conn) = http1::Builder::new()
            .handshake::<_, Full<Bytes>>(TokioIo::new(stream))
            .await
            .map_err(|e| EgressError::Connect(e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.with_upgrades().await;
        });
        self.connects.fetch_add(1, Ordering::Relaxed);
        Ok(sender)
    }

    /// An idle sender that is still usable, or a new connection.
    pub async fn checkout(&self) -> Result<Sender, EgressError> {
        loop {
            let idle = self.pool.lock().ok().and_then(|mut p| p.pop());
            let Some(mut s) = idle else { break };
            if s.is_closed() {
                continue;
            }
            if let Ok(Ok(())) = tokio::time::timeout(Duration::from_millis(500), s.ready()).await {
                return Ok(s);
            }
        }
        self.connect().await
    }

    /// Return a sender after its response was fully read.
    pub fn checkin(&self, s: Sender) {
        if s.is_closed() {
            return;
        }
        if let Ok(mut p) = self.pool.lock()
            && p.len() < self.max_idle
        {
            p.push(s);
        }
    }

    /// Drop every idle connection (after the egress went away).
    pub fn clear(&self) {
        if let Ok(mut p) = self.pool.lock() {
            p.clear();
        }
    }

    /// Send one request; the sender comes back with the response so it can be checked in once the
    /// body has been consumed.
    pub async fn send(
        &self,
        req: Request<Full<Bytes>>,
        header_timeout: Duration,
    ) -> Result<(Response<Incoming>, Sender), EgressError> {
        let mut s = self.checkout().await?;
        self.requests.fetch_add(1, Ordering::Relaxed);
        match tokio::time::timeout(header_timeout, s.send_request(req)).await {
            Err(_) => Err(EgressError::Timeout),
            Ok(Err(e)) => {
                // hyper reports a connection that closed before the request was written as a
                // canceled request (`is_canceled`): never sent, safe to retry.
                if e.is_canceled() {
                    Err(EgressError::Connect(e.to_string()))
                } else {
                    Err(EgressError::Transport(e.to_string()))
                }
            }
            Ok(Ok(resp)) => Ok((resp, s)),
        }
    }

    /// `GET /_root/ceilings` (root/egress.md §4.2).
    pub async fn ceilings(&self, timeout: Duration) -> Result<Value, String> {
        let req = Request::get("/_root/ceilings")
            .header("host", "egress")
            .header(
                "user-agent",
                concat!("init-kernel/", env!("CARGO_PKG_VERSION")),
            )
            .body(Full::new(Bytes::new()))
            .map_err(|e| e.to_string())?;
        let (resp, s) = self
            .send(req, timeout)
            .await
            .map_err(|e| format!("{e:?}"))?;
        let status = resp.status().as_u16();
        let body = tokio::time::timeout(timeout, Limited::new(resp.into_body(), 1 << 20).collect())
            .await
            .map_err(|_| "ceilings: body timeout".to_string())?
            .map_err(|e| e.to_string())?
            .to_bytes();
        self.checkin(s);
        if status != 200 {
            return Err(format!("ceilings: HTTP {status}"));
        }
        serde_json::from_slice(&body).map_err(|e| format!("ceilings: {e}"))
    }
}
