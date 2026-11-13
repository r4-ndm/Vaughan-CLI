//! Client for isolated signer daemon (`vaughan signerd`).
//!
//! Provides [`SignerClient`] to communicate with `vaughan signerd` over a Unix domain socket.
//! Callers submit typed [`SignerIntent`] objects and receive [`SignerResponse`].
//! Raw calldata never crosses the socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::core::signer_intent::SignerIntent;

/// Default socket path for `vaughan signerd`.
pub const DEFAULT_SIGNER_SOCKET: &str = "/run/vaughan/signerd.sock";

/// Environment variable to override the signer socket path.
pub const ENV_SIGNER_SOCKET: &str = "VAUGHAN_SIGNER_SOCKET";

const IPC_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 65536;

/// Resolve the active signer socket path from environment or default.
pub fn resolve_signer_socket_path() -> PathBuf {
    if let Ok(val) = std::env::var(ENV_SIGNER_SOCKET) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    PathBuf::from(DEFAULT_SIGNER_SOCKET)
}

/// Request sent over the Unix socket to `vaughan signerd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignerRequest {
    #[serde(default)]
    pub v: Option<u32>,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<SignerIntent>,
}

/// Response returned by the signer daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignerResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<serde_json::Value>,
}

impl SignerResponse {
    pub fn status(
        address: alloy::primitives::Address,
        chain_id: u64,
        store: String,
        policy_digest: String,
    ) -> Self {
        Self {
            ok: true,
            address: Some(format!("{address:#x}")),
            chain_id: Some(chain_id),
            store: Some(store),
            policy_digest: Some(policy_digest),
            tx_hash: None,
            code: None,
            message: None,
            budget: None,
        }
    }

    pub fn sign_ok(tx_hash: impl Into<String>) -> Self {
        Self {
            ok: true,
            address: None,
            chain_id: None,
            store: None,
            policy_digest: None,
            tx_hash: Some(tx_hash.into()),
            code: None,
            message: None,
            budget: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            address: None,
            chain_id: None,
            store: None,
            policy_digest: None,
            tx_hash: None,
            code: Some(code.into()),
            message: Some(message.into()),
            budget: None,
        }
    }
}

/// Client for communicating with the isolated signer daemon over a Unix domain socket.
#[derive(Debug, Clone)]
pub struct SignerClient {
    socket_path: PathBuf,
}

impl Default for SignerClient {
    fn default() -> Self {
        Self {
            socket_path: resolve_signer_socket_path(),
        }
    }
}

impl SignerClient {
    /// Create a client pointing to an explicit socket path.
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// The socket path this client connects to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Check if the signer daemon is reachable and responding.
    pub async fn ping(&self) -> bool {
        self.status().await.is_ok()
    }

    /// Query status from the signer daemon (`{"op":"status"}`).
    pub async fn status(&self) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "status".to_string(),
            intent: None,
        };
        self.send_request(&req).await
    }

    /// Submit a typed [`SignerIntent`] for verification, policy checking, and signing.
    pub async fn sign(&self, intent: &SignerIntent) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "sign".to_string(),
            intent: Some(intent.clone()),
        };
        self.send_request(&req).await
    }

    /// Submit a typed intent and return the transaction hash on success, or a mapped error string.
    pub async fn execute(&self, intent: &SignerIntent) -> Result<String, String> {
        let resp = self.sign(intent).await?;
        resp.tx_hash
            .ok_or_else(|| "missing tx_hash in signer response".to_string())
    }

    /// Core IPC request handler: connects, sends line, reads line, maps errors.
    async fn send_request(&self, req: &SignerRequest) -> Result<SignerResponse, String> {
        let stream = match timeout(CONNECT_TIMEOUT, UnixStream::connect(&self.socket_path)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(format!(
                    "signer_unavailable: cannot connect to signer socket at {}: {e}",
                    self.socket_path.display()
                ));
            }
            Err(_) => {
                return Err(format!(
                    "signer_unavailable: connection timed out to signer socket at {}",
                    self.socket_path.display()
                ));
            }
        };

        let mut line =
            serde_json::to_string(req).map_err(|e| format!("invalid_request_json: {e}"))?;
        line.push('\n');

        let (reader, mut writer) = stream.into_split();
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("signer_write_failed: {e}"))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("signer_write_failed: {e}"))?;

        let mut buf = BufReader::new(reader);
        let mut response_line = String::new();
        let read_res = timeout(IPC_TIMEOUT, buf.read_line(&mut response_line)).await;
        let bytes_read = match read_res {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(format!("signer_read_failed: {e}")),
            Err(_) => {
                return Err("signer_timeout: request timed out waiting for signer".to_string())
            }
        };

        if bytes_read == 0 {
            return Err("signer_eof: signer closed connection without response".to_string());
        }
        if bytes_read > MAX_RESPONSE_BYTES {
            return Err("signer_response_too_large: response exceeded 64 KiB limit".to_string());
        }

        let resp: SignerResponse = serde_json::from_str(response_line.trim())
            .map_err(|e| format!("invalid_signer_response: {e}"))?;

        if resp.ok {
            Ok(resp)
        } else {
            let code = resp.code.as_deref().unwrap_or("signer_refused");
            let msg = resp.message.as_deref().unwrap_or(code);
            if msg.starts_with(code) {
                Err(msg.to_string())
            } else {
                Err(format!("{code}: {msg}"))
            }
        }
    }
}
