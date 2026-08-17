//! JSON-RPC 2.0 wire types for the provider bridge.
//!
//! EIP-1193 providers speak JSON-RPC 2.0 over WebSocket text frames. This
//! module owns the (de)serialization of requests and responses plus the
//! framing-level errors (parse error, invalid request). Method dispatch and
//! parameter validation happen one layer up, in the request handler.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::codes;

/// A JSON-RPC 2.0 request object.
///
/// `id` is `None` for notifications (no response is sent). `params` is a raw
/// `Value`; each method validates its own shape.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RpcRequest {
    /// Protocol version; `"2.0"` when present.
    pub jsonrpc: Option<String>,
    /// Caller-chosen identifier, echoed verbatim on the response.
    pub id: Option<Value>,
    /// The method name, e.g. `eth_chainId`.
    pub method: String,
    /// Method parameters; defaults to JSON `null` when omitted.
    #[serde(default)]
    pub params: Value,
}

impl RpcRequest {
    /// Parse a raw text frame into a request, mapping framing errors onto
    /// the JSON-RPC `-32700` (parse error) / `-32600` (invalid request) codes.
    ///
    /// Batch arrays (JSON-RPC batches) are rejected as invalid requests:
    /// EIP-1193 provider connections are one request/response per frame, and
    /// the browser clients we bridge to never batch.
    pub fn from_json(raw: &str) -> Result<Self, RpcError> {
        let value: Value = serde_json::from_str(raw).map_err(|_| RpcError::parse_error())?;
        if !value.is_object() {
            return Err(RpcError::invalid_request("request must be a JSON object"));
        }
        let request: Self = serde_json::from_value(value)
            .map_err(|_| RpcError::invalid_request("malformed request"))?;
        if let Some(version) = &request.jsonrpc {
            if version != "2.0" {
                return Err(RpcError::invalid_request("jsonrpc version must be \"2.0\""));
            }
        }
        if request.method.is_empty() {
            return Err(RpcError::invalid_request("method must not be empty"));
        }
        Ok(request)
    }

    /// True when this request carries no `id` and therefore expects no response.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RpcError {
    /// Numeric error code (see [`crate::error::codes`]).
    pub code: i64,
    /// Short, human-readable description. Never contains secret material.
    pub message: String,
    /// Optional structured detail; omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Build an error with the given code and message.
    pub fn new(code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    /// A `-32700` parse error (invalid JSON).
    pub fn parse_error() -> Self {
        Self::new(codes::PARSE_ERROR, "parse error", None)
    }

    /// A `-32600` invalid-request error.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(codes::INVALID_REQUEST, message, None)
    }
}

/// A JSON-RPC 2.0 response object: exactly one of `result` / `error` is set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echo of the request id (`null` for framing errors on notifications).
    pub id: Option<Value>,
    /// The successful result; omitted on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The error object; omitted on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// A successful response for `id` carrying `result`.
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// An error response for `id` carrying `error`.
    pub fn failure(id: Option<Value>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Serialize to a JSON text frame.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("RpcResponse is always serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_ok(raw: &str) -> RpcRequest {
        RpcRequest::from_json(raw).unwrap()
    }

    #[test]
    fn parses_valid_request() {
        let req = parse_ok(r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}"#);
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "eth_chainId");
        assert_eq!(req.params, json!([]));
        assert!(!req.is_notification());
    }

    #[test]
    fn parses_string_and_missing_ids() {
        let req = parse_ok(r#"{"jsonrpc":"2.0","id":"abc","method":"eth_accounts"}"#);
        assert_eq!(req.id, Some(json!("abc")));

        let notification = parse_ok(r#"{"jsonrpc":"2.0","method":"eth_accounts"}"#);
        assert!(notification.is_notification());
    }

    #[test]
    fn params_default_to_null() {
        let req = parse_ok(r#"{"id":2,"method":"eth_chainId"}"#);
        assert_eq!(req.params, Value::Null);
    }

    #[test]
    fn rejects_malformed_json_with_parse_error() {
        let err = RpcRequest::from_json("{not json").unwrap_err();
        assert_eq!(err.code, codes::PARSE_ERROR);
    }

    #[test]
    fn rejects_batches_and_scalars_as_invalid_request() {
        let batch = RpcRequest::from_json(r#"[{"id":1,"method":"x"}]"#).unwrap_err();
        assert_eq!(batch.code, codes::INVALID_REQUEST);

        let scalar = RpcRequest::from_json(r#"42"#).unwrap_err();
        assert_eq!(scalar.code, codes::INVALID_REQUEST);
    }

    #[test]
    fn rejects_wrong_version_and_empty_method() {
        assert_eq!(
            RpcRequest::from_json(r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#)
                .unwrap_err()
                .code,
            codes::INVALID_REQUEST
        );
        assert_eq!(
            RpcRequest::from_json(r#"{"id":1,"method":""}"#)
                .unwrap_err()
                .code,
            codes::INVALID_REQUEST
        );
    }

    #[test]
    fn response_round_trip() {
        let response = RpcResponse::success(Some(json!(7)), json!("0x1"));
        let raw = response.to_json();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert_eq!(value["result"], "0x1");
        assert!(value.get("error").is_none());
    }

    #[test]
    fn error_response_shape() {
        let response = RpcResponse::failure(
            Some(json!("abc")),
            RpcError::new(4001, "user rejected", None),
        );
        let value: Value = serde_json::from_str(&response.to_json()).unwrap();
        assert_eq!(value["id"], "abc");
        assert_eq!(value["error"]["code"], 4001);
        assert_eq!(value["error"]["message"], "user rejected");
        assert!(value.get("result").is_none());
        assert!(value["error"].get("data").is_none());
    }
}
