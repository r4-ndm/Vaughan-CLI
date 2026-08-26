//! EIP-1193 method implementations (FR-2.2).
//!
//! [`Eip1193Handler`] turns validated JSON-RPC requests into typed calls on a
//! [`WalletHandle`] supplied by the host (the TUI). Everything EIP-1193
//! specific lives here — param shapes, quantity normalization (hex → decimal
//! strings for `vaughan-core`), and light address checks. The host owns
//! wallet state, approval prompts, and signing; the handler never signs.
//!
//! Supported methods (FR-2.2 + `vaughan_signTransaction` for the Freedom
//! Browser signer backend, see `docs/freedom-browser-integration.md` §4):
//!
//! - `eth_accounts`, `eth_requestAccounts` — account list for the caller
//! - `eth_chainId` — active chain id as `0x` hex
//! - `eth_sendTransaction` — sign **and broadcast**, returns tx hash
//! - `vaughan_signTransaction` — sign **only**, returns raw signed tx
//! - `personal_sign` — EIP-191 signature over a message
//! - `eth_signTypedData_v4` — EIP-712 signature over typed data
//! - `wallet_switchEthereumChain` — switch to a built-in network

use std::sync::Arc;

use alloy::primitives::U256;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::error::ProviderError;
use crate::handler::{HandlerResult, RequestCtx, RequestHandler};
use crate::rpc::RpcRequest;

/// EIP-1193 transaction parameters (`eth_sendTransaction` / `vaughan_signTransaction`).
///
/// Quantities are accepted as `0x` hex (EIP-1193) or decimal digits and are
/// normalized to decimal strings before the host sees them, matching what
/// `vaughan-core`'s transaction builder expects.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TxParams {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub gas: Option<String>,
    #[serde(default, rename = "gasPrice")]
    pub gas_price: Option<String>,
    #[serde(default, rename = "maxFeePerGas")]
    pub max_fee_per_gas: Option<String>,
    #[serde(default, rename = "maxPriorityFeePerGas")]
    pub max_priority_fee_per_gas: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default, rename = "chainId")]
    pub chain_id: Option<String>,
}

impl TxParams {
    /// Convert every quantity field to a canonical decimal string.
    ///
    /// Rejects malformed quantities with `-32602` so the host never has to
    /// cope with bad wire data.
    pub fn normalize_quantities(&mut self) -> Result<(), ProviderError> {
        let fields = [
            &mut self.value,
            &mut self.gas,
            &mut self.gas_price,
            &mut self.max_fee_per_gas,
            &mut self.max_priority_fee_per_gas,
            &mut self.nonce,
            &mut self.chain_id,
        ];
        for field in fields {
            if let Some(raw) = field {
                *field = Some(quantity_to_decimal(raw)?);
            }
        }
        Ok(())
    }

    /// Light address sanity check on `from`/`to` (0x + 40 hex chars).
    ///
    /// Full checksum validation happens in the wallet core at build time;
    /// this only fails fast on obviously malformed input.
    pub fn validate_addresses(&self) -> Result<(), ProviderError> {
        for address in [&self.from, &self.to].into_iter().flatten() {
            if !is_address_like(address) {
                return Err(ProviderError::InvalidParams(format!(
                    "invalid address: {address}"
                )));
            }
        }
        Ok(())
    }
}

/// Parse a hex (`0x…`) or decimal quantity into a canonical decimal string.
pub fn quantity_to_decimal(raw: &str) -> Result<String, ProviderError> {
    if raw.is_empty() {
        return Err(ProviderError::InvalidParams("empty quantity".into()));
    }
    let parsed = if let Some(hex) = raw.strip_prefix("0x") {
        if hex.is_empty() {
            return Err(ProviderError::InvalidParams(format!(
                "invalid quantity: {raw}"
            )));
        }
        U256::from_str_radix(hex, 16)
    } else if raw.bytes().all(|b| b.is_ascii_digit()) {
        U256::from_str_radix(raw, 10)
    } else {
        return Err(ProviderError::InvalidParams(format!(
            "invalid quantity: {raw}"
        )));
    };
    parsed
        .map(|value| value.to_string())
        .map_err(|_| ProviderError::InvalidParams(format!("invalid quantity: {raw}")))
}

/// True for `0x`-prefixed 40-hex-char addresses (case-insensitive here;
/// checksum is verified by the wallet core).
pub fn is_address_like(address: &str) -> bool {
    let Some(hex) = address.strip_prefix("0x") else {
        return false;
    };
    hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The host-side wallet contract the TUI implements.
///
/// Every signing method must go through an explicit user approval before
/// touching key material (FR-2.3); the host decides how (and whether) to
/// prompt based on the request context (origin, method, payload).
#[async_trait]
pub trait WalletHandle: Send + Sync + 'static {
    /// Accounts the caller may use; `[]` when locked or unapproved.
    async fn accounts(&self, ctx: &RequestCtx) -> Result<Vec<String>, ProviderError>;

    /// Connect gesture (`eth_requestAccounts`): returns the accounts after
    /// any host-side approval.
    async fn request_accounts(&self, ctx: &RequestCtx) -> Result<Vec<String>, ProviderError>;

    /// The active chain id as a `0x` hex string.
    async fn chain_id(&self, ctx: &RequestCtx) -> Result<String, ProviderError>;

    /// Sign and broadcast `tx`; returns the tx hash. Must be user-approved.
    async fn send_transaction(
        &self,
        ctx: &RequestCtx,
        tx: TxParams,
    ) -> Result<String, ProviderError>;

    /// Sign `tx` without broadcasting; returns the raw signed tx (0x hex).
    /// Must be user-approved. Serves the Freedom Browser `Signer` contract.
    async fn sign_transaction(
        &self,
        ctx: &RequestCtx,
        tx: TxParams,
    ) -> Result<String, ProviderError>;

    /// EIP-191 signature (`personal_sign`); `message` is passed through
    /// unmodified — the host decodes `0x`-hex to bytes before signing.
    async fn sign_message(
        &self,
        ctx: &RequestCtx,
        address: &str,
        message: &str,
    ) -> Result<String, ProviderError>;

    /// EIP-712 signature (`eth_signTypedData_v4`) over the parsed payload
    /// (`{types, primaryType, domain, message}`).
    async fn sign_typed_data(
        &self,
        ctx: &RequestCtx,
        address: &str,
        typed_data: Value,
    ) -> Result<String, ProviderError>;

    /// Switch the active network; `chain_id` is a decimal string. Unknown
    /// chains return [`ProviderError::UnrecognizedChain`] (4902).
    async fn switch_chain(&self, ctx: &RequestCtx, chain_id: &str) -> Result<(), ProviderError>;
}

/// The EIP-1193 request dispatcher.
///
/// Wire one of these around an `Arc<dyn WalletHandle>` and pass it to
/// [`crate::server::ProviderServer::serve`].
pub struct Eip1193Handler<W> {
    wallet: Arc<W>,
}

impl<W> Eip1193Handler<W> {
    pub fn new(wallet: Arc<W>) -> Self {
        Self { wallet }
    }
}

#[async_trait]
impl<W: WalletHandle> RequestHandler for Eip1193Handler<W> {
    async fn handle(&self, ctx: RequestCtx, request: RpcRequest) -> HandlerResult {
        let wallet = &self.wallet;
        match request.method.as_str() {
            "eth_accounts" => {
                let accounts = wallet.accounts(&ctx).await?;
                Ok(serde_json::json!(accounts))
            }
            "eth_requestAccounts" => {
                let accounts = wallet.request_accounts(&ctx).await?;
                Ok(serde_json::json!(accounts))
            }
            "eth_chainId" => Ok(Value::String(wallet.chain_id(&ctx).await?)),
            "eth_sendTransaction" => {
                let mut tx = parse_tx_params(&request.params, "eth_sendTransaction")?;
                tx.normalize_quantities()?;
                tx.validate_addresses()?;
                Ok(Value::String(wallet.send_transaction(&ctx, tx).await?))
            }
            "vaughan_signTransaction" => {
                let mut tx = parse_tx_params(&request.params, "vaughan_signTransaction")?;
                tx.normalize_quantities()?;
                tx.validate_addresses()?;
                Ok(Value::String(wallet.sign_transaction(&ctx, tx).await?))
            }
            "personal_sign" => {
                let (message, address) = parse_personal_sign(&request.params)?;
                Ok(Value::String(
                    wallet.sign_message(&ctx, &address, &message).await?,
                ))
            }
            "eth_signTypedData_v4" => {
                let (address, typed_data) = parse_sign_typed_data(&request.params)?;
                Ok(Value::String(
                    wallet.sign_typed_data(&ctx, &address, typed_data).await?,
                ))
            }
            "wallet_switchEthereumChain" => {
                let chain_id = parse_switch_chain(&request.params)?;
                wallet.switch_chain(&ctx, &chain_id).await?;
                Ok(Value::Null)
            }
            other => Err(ProviderError::UnsupportedMethod(other.to_string())),
        }
    }
}

// ---- param parsing helpers (all failures map to -32602) ----

/// The first element of a params array, or an invalid-params error.
fn first_param<'a>(params: &'a Value, method: &str) -> Result<&'a Value, ProviderError> {
    let Value::Array(array) = params else {
        return Err(ProviderError::InvalidParams(format!(
            "{method}: params must be an array"
        )));
    };
    array
        .first()
        .ok_or_else(|| ProviderError::InvalidParams(format!("{method}: missing params")))
}

/// Parse the transaction object for `eth_sendTransaction` /
/// `vaughan_signTransaction`.
fn parse_tx_params(params: &Value, method: &str) -> Result<TxParams, ProviderError> {
    let tx = first_param(params, method)?;
    serde_json::from_value(tx.clone())
        .map_err(|e| ProviderError::InvalidParams(format!("{method}: {e}")))
}

/// Parse `[message, address]` for `personal_sign`.
fn parse_personal_sign(params: &Value) -> Result<(String, String), ProviderError> {
    let Value::Array(array) = params else {
        return Err(ProviderError::InvalidParams(
            "personal_sign: params must be an array".into(),
        ));
    };
    if array.len() < 2 {
        return Err(ProviderError::InvalidParams(
            "personal_sign: expected [message, address]".into(),
        ));
    }
    let first = array[0]
        .as_str()
        .ok_or_else(|| {
            ProviderError::InvalidParams("personal_sign: params must be strings".into())
        })?
        .to_string();
    let second = array[1]
        .as_str()
        .ok_or_else(|| {
            ProviderError::InvalidParams("personal_sign: params must be strings".into())
        })?
        .to_string();

    // Standard EIP-1193 order is `[message, address]`, but legacy web3.js /
    // older dApps send `[address, message]`. Auto-detect by shape: an address
    // in the first slot means the legacy order.
    let (message, address) = if is_address_like(&first) {
        (second, first)
    } else {
        (first, second)
    };
    if !is_address_like(&address) {
        return Err(ProviderError::InvalidParams(format!(
            "personal_sign: invalid address: {address}"
        )));
    }
    Ok((message, address))
}

/// Parse `[address, typedData]` for `eth_signTypedData_v4`.
///
/// `typedData` may arrive as an object (standard) or a JSON string (older
/// dApps); both are normalized to the parsed object.
fn parse_sign_typed_data(params: &Value) -> Result<(String, Value), ProviderError> {
    let Value::Array(array) = params else {
        return Err(ProviderError::InvalidParams(
            "eth_signTypedData_v4: params must be an array".into(),
        ));
    };
    if array.len() < 2 {
        return Err(ProviderError::InvalidParams(
            "eth_signTypedData_v4: expected [address, typedData]".into(),
        ));
    }
    let address = array[0]
        .as_str()
        .ok_or_else(|| {
            ProviderError::InvalidParams("eth_signTypedData_v4: address must be a string".into())
        })?
        .to_string();
    if !is_address_like(&address) {
        return Err(ProviderError::InvalidParams(format!(
            "eth_signTypedData_v4: invalid address: {address}"
        )));
    }
    let typed_data = match &array[1] {
        Value::Object(_) => array[1].clone(),
        Value::String(raw) => serde_json::from_str(raw).map_err(|e| {
            ProviderError::InvalidParams(format!(
                "eth_signTypedData_v4: typed data is not valid JSON: {e}"
            ))
        })?,
        _ => {
            return Err(ProviderError::InvalidParams(
                "eth_signTypedData_v4: typed data must be an object".into(),
            ));
        }
    };
    Ok((address, typed_data))
}

/// Parse `[{chainId}]` for `wallet_switchEthereumChain`; returns the chain id
/// as a decimal string.
fn parse_switch_chain(params: &Value) -> Result<String, ProviderError> {
    let request = first_param(params, "wallet_switchEthereumChain")?;
    let chain_id = request
        .get("chainId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::InvalidParams(
                "wallet_switchEthereumChain: missing chainId string".into(),
            )
        })?;
    quantity_to_decimal(chain_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quantity_hex_to_decimal() {
        assert_eq!(quantity_to_decimal("0x0").unwrap(), "0");
        assert_eq!(quantity_to_decimal("0x1").unwrap(), "1");
        assert_eq!(
            quantity_to_decimal("0xde0b6b3a7640000").unwrap(),
            "1000000000000000000"
        );
        assert_eq!(quantity_to_decimal("12345").unwrap(), "12345");
        assert!(quantity_to_decimal("0x").is_err());
        assert!(quantity_to_decimal("0xzz").is_err());
        assert!(quantity_to_decimal("12.5").is_err());
        assert!(quantity_to_decimal("-1").is_err());
        assert!(quantity_to_decimal("").is_err());
    }

    #[test]
    fn address_like_check() {
        assert!(is_address_like(
            "0x9858effd232b4033e47d90003d41ec34ecaeda94"
        ));
        assert!(is_address_like(
            "0x9858EFFD232B4033E47D90003D41EC34ECAEDA94"
        ));
        assert!(!is_address_like("0x1234"));
        assert!(!is_address_like("9858effd232b4033e47d90003d41ec34ecaeda94"));
        assert!(!is_address_like("not-an-address"));
    }

    #[test]
    fn tx_params_normalize_quantities() {
        let mut tx: TxParams = serde_json::from_value(json!({
            "from": "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            "to": "0x0000000000000000000000000000000000000000",
            "value": "0xde0b6b3a7640000",
            "gas": "0x5208",
            "maxFeePerGas": "0x77359400",
            "data": "0x1234",
        }))
        .unwrap();
        tx.normalize_quantities().unwrap();
        tx.validate_addresses().unwrap();
        assert_eq!(tx.value.as_deref(), Some("1000000000000000000"));
        assert_eq!(tx.gas.as_deref(), Some("21000"));
        assert_eq!(tx.max_fee_per_gas.as_deref(), Some("2000000000"));
        assert_eq!(tx.data.as_deref(), Some("0x1234"));
        assert_eq!(tx.gas_price, None);
    }

    #[test]
    fn tx_params_reject_bad_quantity_and_address() {
        let mut bad_quantity: TxParams = serde_json::from_value(json!({
            "to": "0x0000000000000000000000000000000000000000",
            "value": "0xnothex",
        }))
        .unwrap();
        assert!(bad_quantity.normalize_quantities().is_err());

        let bad_address: TxParams = serde_json::from_value(json!({
            "to": "0x123",
        }))
        .unwrap();
        assert!(bad_address.validate_addresses().is_err());
    }

    #[test]
    fn personal_sign_params() {
        let (message, address) = parse_personal_sign(&json!([
            "0x68656c6c6f",
            "0x9858effd232b4033e47d90003d41ec34ecaeda94"
        ]))
        .unwrap();
        assert_eq!(message, "0x68656c6c6f");
        assert_eq!(address, "0x9858effd232b4033e47d90003d41ec34ecaeda94");

        assert!(parse_personal_sign(&json!(["0x68656c6c6f"])).is_err());
        assert!(parse_personal_sign(&json!(["0x68656c6c6f", "not-an-address"])).is_err());
        assert!(parse_personal_sign(&json!({})).is_err());
    }

    #[test]
    fn personal_sign_accepts_legacy_address_first_order() {
        // Legacy web3.js order: [address, message].
        let (message, address) = parse_personal_sign(&json!([
            "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            "0x68656c6c6f"
        ]))
        .unwrap();
        assert_eq!(message, "0x68656c6c6f");
        assert_eq!(address, "0x9858effd232b4033e47d90003d41ec34ecaeda94");
    }

    #[test]
    fn typed_data_params_accepts_object_and_json_string() {
        let payload = json!({
            "types": {"EIP712Domain": [], "Message": [{"name": "hello", "type": "string"}]},
            "primaryType": "Message",
            "domain": {"name": "Example"},
            "message": {"hello": "world"},
        });
        let address = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

        let (addr, data) = parse_sign_typed_data(&json!([address, payload.clone()])).unwrap();
        assert_eq!(addr, address);
        assert_eq!(data, payload);

        let (_, data) = parse_sign_typed_data(&json!([address, payload.to_string()])).unwrap();
        assert_eq!(data, payload);

        assert!(parse_sign_typed_data(&json!([address, 42])).is_err());
        assert!(parse_sign_typed_data(&json!([address, "not json"])).is_err());
    }

    #[test]
    fn switch_chain_params() {
        assert_eq!(
            parse_switch_chain(&json!([{"chainId": "0x171"}])).unwrap(),
            "369"
        );
        assert_eq!(
            parse_switch_chain(&json!([{"chainId": "943"}])).unwrap(),
            "943"
        );
        assert!(parse_switch_chain(&json!([{}])).is_err());
        assert!(parse_switch_chain(&json!([])).is_err());
        assert!(parse_switch_chain(&json!([{"chainId": "0xzz"}])).is_err());
    }

    // ---- dispatch tests against a fake wallet ----

    #[derive(Clone)]
    struct FakeWallet {
        account: String,
        chain: String,
        reject: Option<ProviderError>,
    }

    #[async_trait]
    impl WalletHandle for FakeWallet {
        async fn accounts(&self, _ctx: &RequestCtx) -> Result<Vec<String>, ProviderError> {
            Ok(vec![self.account.clone()])
        }
        async fn request_accounts(&self, _ctx: &RequestCtx) -> Result<Vec<String>, ProviderError> {
            Ok(vec![self.account.clone()])
        }
        async fn chain_id(&self, _ctx: &RequestCtx) -> Result<String, ProviderError> {
            Ok(self.chain.clone())
        }
        async fn send_transaction(
            &self,
            _ctx: &RequestCtx,
            tx: TxParams,
        ) -> Result<String, ProviderError> {
            if let Some(reject) = &self.reject {
                return Err(reject.clone());
            }
            Ok(format!("0xhash-{}", tx.value.as_deref().unwrap_or("0")))
        }
        async fn sign_transaction(
            &self,
            _ctx: &RequestCtx,
            tx: TxParams,
        ) -> Result<String, ProviderError> {
            Ok(format!("0xsigned-{}", tx.value.as_deref().unwrap_or("0")))
        }
        async fn sign_message(
            &self,
            _ctx: &RequestCtx,
            address: &str,
            message: &str,
        ) -> Result<String, ProviderError> {
            Ok(format!("0xsig-{address}-{message}"))
        }
        async fn sign_typed_data(
            &self,
            _ctx: &RequestCtx,
            address: &str,
            typed_data: Value,
        ) -> Result<String, ProviderError> {
            let name = typed_data["domain"]["name"].as_str().unwrap_or("?");
            Ok(format!("0xtyped-{address}-{name}"))
        }
        async fn switch_chain(
            &self,
            _ctx: &RequestCtx,
            chain_id: &str,
        ) -> Result<(), ProviderError> {
            if chain_id == "369" {
                Ok(())
            } else {
                Err(ProviderError::UnrecognizedChain(chain_id.into()))
            }
        }
    }

    fn ctx() -> RequestCtx {
        RequestCtx {
            peer: "127.0.0.1:9999".parse().unwrap(),
            origin: None,
            page_origin: None,
        }
    }

    async fn dispatch(wallet: &FakeWallet, raw: &str) -> HandlerResult {
        let handler = Eip1193Handler::new(Arc::new(wallet.clone()));
        let request = RpcRequest::from_json(raw).unwrap();
        handler.handle(ctx(), request).await
    }

    fn fake_wallet() -> FakeWallet {
        FakeWallet {
            account: "0x9858effd232b4033e47d90003d41ec34ecaeda94".into(),
            chain: "0x171".into(),
            reject: None,
        }
    }

    #[tokio::test]
    async fn dispatches_read_methods() {
        let wallet = fake_wallet();
        assert_eq!(
            dispatch(&wallet, r#"{"id":1,"method":"eth_accounts"}"#)
                .await
                .unwrap(),
            json!([wallet.account])
        );
        assert_eq!(
            dispatch(&wallet, r#"{"id":2,"method":"eth_requestAccounts"}"#)
                .await
                .unwrap(),
            json!([wallet.account])
        );
        assert_eq!(
            dispatch(&wallet, r#"{"id":3,"method":"eth_chainId"}"#)
                .await
                .unwrap(),
            json!("0x171")
        );
    }

    #[tokio::test]
    async fn dispatches_send_and_sign_methods() {
        let wallet = fake_wallet();
        let account = &wallet.account;

        let send = dispatch(
            &wallet,
            r#"{"id":1,"method":"eth_sendTransaction","params":[{"to":"0x0000000000000000000000000000000000000000","value":"0x1"}]}"#,
        )
        .await
        .unwrap();
        assert_eq!(send, json!("0xhash-1"));

        let sign = dispatch(
            &wallet,
            r#"{"id":2,"method":"vaughan_signTransaction","params":[{"to":"0x0000000000000000000000000000000000000000","value":"0xde0b6b3a7640000"}]}"#,
        )
        .await
        .unwrap();
        assert_eq!(sign, json!("0xsigned-1000000000000000000"));

        let message = dispatch(
            &wallet,
            &format!(
                r#"{{"id":3,"method":"personal_sign","params":["0x68656c6c6f","{account}"]}}"#
            ),
        )
        .await
        .unwrap();
        assert_eq!(message, json!(format!("0xsig-{account}-0x68656c6c6f")));

        let typed = dispatch(
            &wallet,
            &format!(
                r#"{{"id":4,"method":"eth_signTypedData_v4","params":["{account}",{{"types":{{"EIP712Domain":[]}},"primaryType":"x","domain":{{"name":"Example"}},"message":{{}}}}]}}"#
            ),
        )
        .await
        .unwrap();
        assert_eq!(typed, json!(format!("0xtyped-{account}-Example")));
    }

    #[tokio::test]
    async fn dispatches_switch_chain() {
        let wallet = fake_wallet();
        assert_eq!(
            dispatch(
                &wallet,
                r#"{"id":1,"method":"wallet_switchEthereumChain","params":[{"chainId":"0x171"}]}"#
            )
            .await
            .unwrap(),
            Value::Null
        );
        let err = dispatch(
            &wallet,
            r#"{"id":2,"method":"wallet_switchEthereumChain","params":[{"chainId":"0x1"}]}"#,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProviderError::UnrecognizedChain(_)));
        assert_eq!(err.code(), 4902);
    }

    #[tokio::test]
    async fn unknown_method_is_4200() {
        let wallet = fake_wallet();
        let err = dispatch(&wallet, r#"{"id":1,"method":"eth_foo"}"#)
            .await
            .unwrap_err();
        assert_eq!(err.code(), 4200);
        assert!(matches!(err, ProviderError::UnsupportedMethod(m) if m == "eth_foo"));
    }

    #[tokio::test]
    async fn malformed_params_are_32602() {
        let wallet = fake_wallet();
        let err = dispatch(
            &wallet,
            r#"{"id":1,"method":"eth_sendTransaction","params":[]}"#,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), -32602);

        let err = dispatch(
            &wallet,
            r#"{"id":2,"method":"eth_sendTransaction","params":[{"to":"0x123","value":"0x1"}]}"#,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), -32602);
    }

    #[tokio::test]
    async fn host_errors_pass_through() {
        let mut wallet = fake_wallet();
        wallet.reject = Some(ProviderError::UserRejected);
        let err = dispatch(
            &wallet,
            r#"{"id":1,"method":"eth_sendTransaction","params":[{"to":"0x0000000000000000000000000000000000000000","value":"0x1"}]}"#,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProviderError::UserRejected));
    }

    #[tokio::test]
    async fn end_to_end_over_real_websocket() {
        use crate::events::ProviderEvent;
        use crate::server::ProviderServer;
        use futures_util::{SinkExt, StreamExt};
        use std::time::Duration;
        use tokio::time::timeout;
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::protocol::Message;

        // Full stack: real WS socket → Eip1193Handler dispatch → WalletHandle.
        let handler = Eip1193Handler::new(Arc::new(fake_wallet()));
        let server = ProviderServer::bind(0).await.unwrap();
        let url = server.url();
        let events = crate::events::EventBus::new();
        let task = tokio::spawn(server.serve(Arc::new(handler), events.clone()));

        let (mut ws, _) = connect_async(&url).await.unwrap();

        // Read method over the wire.
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected a text reply");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["result"], "0x171");

        // Signing method over the wire.
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":2,"method":"personal_sign","params":["0x68656c6c6f","0x9858effd232b4033e47d90003d41ec34ecaeda94"]}"#
                .into(),
        ))
        .await
        .unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected a text reply");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["result"].as_str().unwrap().starts_with("0xsig-"));

        // Unsupported method → EIP-1193 4200.
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":3,"method":"eth_mining"}"#.into(),
        ))
        .await
        .unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected a text reply");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["error"]["code"], 4200);

        // Host-published event is relayed as a JSON-RPC notification.
        events.publish(ProviderEvent::AccountsChanged(vec!["0xabc".into()]));
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected a text reply");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["method"], "accountsChanged");
        assert_eq!(value["params"][0], "0xabc");

        task.abort();
    }
}
