//! Client for isolated signer daemon (`vaughan signerd`).
//!
//! Provides [`SignerClient`] to communicate with `vaughan signerd` over a Unix domain socket.
//! Callers submit typed [`SignerIntent`] objects and receive [`SignerResponse`].
//! Raw calldata never crosses the socket.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::de::{self, Deserializer, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use zeroize::Zeroize;

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

mod opt_secret_string {
    use super::*;

    struct SecretStringVisitor;

    impl<'de> Visitor<'de> for SecretStringVisitor {
        type Value = SecretString;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a password string")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SecretString::from(v.to_owned()))
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SecretString::from(v))
        }
    }

    struct OptSecretStringVisitor;

    impl<'de> Visitor<'de> for OptSecretStringVisitor {
        type Value = Option<SecretString>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an optional password string")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer
                .deserialize_string(SecretStringVisitor)
                .map(Some)
        }
    }

    pub fn serialize<S>(val: &Option<SecretString>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match val {
            Some(s) => serializer.serialize_some(s.expose_secret()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_option(OptSecretStringVisitor)
    }
}

/// Request sent over the Unix socket to `vaughan signerd`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u32>,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<SignerIntent>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_secret_string"
    )]
    pub password: Option<SecretString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
}

impl fmt::Debug for SignerRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignerRequest")
            .field("v", &self.v)
            .field("op", &self.op)
            .field("intent", &self.intent)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("policy", &self.policy)
            .field("expected_digest", &self.expected_digest)
            .field("auth", &self.auth)
            .finish()
    }
}

/// Read-only status of a staged policy update returned by `{"op":"policy_pending"}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingInfo {
    /// Canonical SHA-256 hex digest of the staged pending policy.
    pub policy_digest: String,
    /// Unix timestamp when the staged pending policy expires.
    pub expires_at_unix: u64,
    /// True only when a valid, trusted, unexpired `policy-approval.json` is present.
    pub approval_present: bool,
}

/// Response returned by the signer daemon.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub policy: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<serde_json::Value>,
    /// Read-only staged pending policy status (populated by `policy_pending` and `stage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingInfo>,
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
            ..Default::default()
        }
    }

    pub fn policy_ok(policy: serde_json::Value, policy_digest: String) -> Self {
        Self {
            ok: true,
            policy_digest: Some(policy_digest),
            policy: Some(policy),
            ..Default::default()
        }
    }

    pub fn policy_update_ok(policy_digest: String) -> Self {
        Self {
            ok: true,
            policy_digest: Some(policy_digest),
            ..Default::default()
        }
    }

    /// Construct a `{"op":"policy_pending"}` response.
    pub fn policy_pending_ok(pending: Option<PendingInfo>) -> Self {
        Self {
            ok: true,
            pending,
            ..Default::default()
        }
    }

    pub fn step_up_required(policy_digest: String) -> Self {
        Self {
            ok: false,
            policy_digest: Some(policy_digest),
            code: Some("step_up_required".to_string()),
            message: Some("run: sudo vaughan signerd approve".to_string()),
            ..Default::default()
        }
    }

    pub fn sign_ok(tx_hash: impl Into<String>) -> Self {
        Self {
            ok: true,
            tx_hash: Some(tx_hash.into()),
            ..Default::default()
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            code: Some(code.into()),
            message: Some(message.into()),
            ..Default::default()
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
            password: None,
            policy: None,
            expected_digest: None,
            auth: None,
        };
        self.send_request(&req).await
    }

    /// Query current policy and digest from the signer daemon (`{"op":"policy"}`).
    pub async fn policy(&self) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "policy".to_string(),
            intent: None,
            password: None,
            policy: None,
            expected_digest: None,
            auth: None,
        };
        self.send_request(&req).await
    }

    /// Update the active policy on the signer daemon with vault password step-up (`{"op":"policy_update"}`).
    pub async fn policy_update(
        &self,
        password: SecretString,
        policy: serde_json::Value,
        expected_digest: Option<String>,
    ) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "policy_update".to_string(),
            intent: None,
            password: Some(password),
            policy: Some(policy),
            expected_digest,
            auth: None,
        };
        self.send_request(&req).await
    }

    /// Query read-only staged policy status (`{"op":"policy_pending"}`).
    ///
    /// Returns the raw [`SignerResponse`] so callers can inspect structured daemon errors separately
    /// from transport failures.
    pub async fn policy_pending(&self) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "policy_pending".to_string(),
            intent: None,
            password: None,
            policy: None,
            expected_digest: None,
            auth: None,
        };
        self.send_raw_request(&req).await
    }

    /// Stage a policy update for out-of-band root approval (`{"op":"policy_update","auth":"stage"}`).
    ///
    /// Returns the raw [`SignerResponse`] (note that staging returns `ok: false` with
    /// `code == "step_up_required"`).
    pub async fn policy_stage(
        &self,
        policy: serde_json::Value,
        expected_digest: Option<String>,
    ) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "policy_update".to_string(),
            intent: None,
            password: None,
            policy: Some(policy),
            expected_digest,
            auth: Some("stage".to_string()),
        };
        self.send_raw_request(&req).await
    }

    /// Apply a previously staged policy update after root approval (`{"op":"policy_update","auth":"approved"}`).
    ///
    /// Returns the raw [`SignerResponse`] so callers can distinguish `step_up_failed` from
    /// transport failures.
    pub async fn policy_approved(
        &self,
        policy: serde_json::Value,
        expected_digest: Option<String>,
    ) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "policy_update".to_string(),
            intent: None,
            password: None,
            policy: Some(policy),
            expected_digest,
            auth: Some("approved".to_string()),
        };
        self.send_raw_request(&req).await
    }

    /// Submit a typed [`SignerIntent`] for verification, policy checking, and signing.
    pub async fn sign(&self, intent: &SignerIntent) -> Result<SignerResponse, String> {
        let req = SignerRequest {
            v: Some(1),
            op: "sign".to_string(),
            intent: Some(intent.clone()),
            password: None,
            policy: None,
            expected_digest: None,
            auth: None,
        };
        self.send_request(&req).await
    }

    /// Submit a typed intent and return the transaction hash on success, or a mapped error string.
    pub async fn execute(&self, intent: &SignerIntent) -> Result<String, String> {
        let resp = self.sign(intent).await?;
        resp.tx_hash
            .ok_or_else(|| "missing tx_hash in signer response".to_string())
    }

    async fn connect_socket(&self) -> Result<UnixStream, String> {
        match timeout(CONNECT_TIMEOUT, UnixStream::connect(&self.socket_path)).await {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(format!(
                "signer_unavailable: cannot connect to signer socket at {}: {e}",
                self.socket_path.display()
            )),
            Err(_) => Err(format!(
                "signer_unavailable: connection timed out to signer socket at {}",
                self.socket_path.display()
            )),
        }
    }

    /// Send a request and decode the raw [`SignerResponse`] without converting `ok: false` into `Err`.
    async fn send_raw_request(&self, req: &SignerRequest) -> Result<SignerResponse, String> {
        let stream = self.connect_socket().await?;
        let mut line =
            serde_json::to_string(req).map_err(|e| format!("invalid_request_json: {e}"))?;
        line.push('\n');

        let (reader, mut writer) = stream.into_split();
        let write_res = async {
            writer
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("signer_write_failed: {e}"))?;
            writer
                .flush()
                .await
                .map_err(|e| format!("signer_write_failed: {e}"))
        }
        .await;
        line.zeroize();
        write_res?;

        let mut buf = BufReader::new(reader);
        let mut response_line = String::new();
        let bytes_read = match timeout(IPC_TIMEOUT, buf.read_line(&mut response_line)).await {
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

        serde_json::from_str(response_line.trim())
            .map_err(|e| format!("invalid_signer_response: {e}"))
    }

    /// Core IPC request handler: connects, sends line, reads line, maps daemon errors to `Err`.
    async fn send_request(&self, req: &SignerRequest) -> Result<SignerResponse, String> {
        let resp = self.send_raw_request(req).await?;
        if resp.ok {
            return Ok(resp);
        }
        let code = resp.code.as_deref().unwrap_or("signer_refused");
        let msg = resp.message.as_deref().unwrap_or(code);
        if msg.starts_with(code) {
            Err(msg.to_string())
        } else {
            Err(format!("{code}: {msg}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signer_request_debug_redacts_password() {
        let secret = "SuperSecretVaultPw!99";
        let req = SignerRequest {
            v: Some(1),
            op: "policy_update".to_string(),
            intent: None,
            password: Some(SecretString::from(secret.to_string())),
            policy: Some(serde_json::json!({ "daily_cap_pct": 50 })),
            expected_digest: Some("abc123".to_string()),
            auth: None,
        };
        let rendered = format!("{req:?}");
        assert!(
            rendered.contains("[REDACTED]"),
            "expected [REDACTED] in Debug output: {rendered}"
        );
        assert!(
            !rendered.contains(secret),
            "plaintext secret leaked into Debug output: {rendered}"
        );
    }

    #[test]
    fn signer_request_rejects_unknown_top_level_fields() {
        let raw = r#"{"op":"policy","unexpected_field":123}"#;
        let parsed: Result<SignerRequest, _> = serde_json::from_str(raw);
        assert!(
            parsed.is_err(),
            "SignerRequest must reject unknown top-level fields"
        );
    }
}
