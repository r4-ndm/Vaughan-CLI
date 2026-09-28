//! Sync USB helpers for Trezor (`trezor-client`).
//!
//! All HID I/O is blocking — callers wrap with `spawn_blocking` / worker threads.
//! Trezor One PIN matrix uses [`super::ui_bridge::TrezorUiBridge`] (never logged).

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope, TxLegacy};
use alloy::eips::eip2718::Encodable2718;
use alloy::primitives::{Address, Bytes, Signature as AlloySignature, TxKind, U256};
use secrecy::{ExposeSecret, SecretString};
use trezor_client::client::{Signature as TrezorSignature, Trezor};
use trezor_client::protos;
use trezor_client::{TrezorMessage, TrezorResponse};

use crate::chains::EvmTransaction;
use crate::error::WalletError;
use crate::security::signing::Eip712Hashes;

use super::eip712::TypedDataStream;
use super::ui_bridge::TrezorUiBridge;

/// Map Trezor/USB errors into wallet-facing messages (no secrets).
pub(crate) fn map_trezor(err: impl std::fmt::Display) -> WalletError {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if lower.contains("no device")
        || lower.contains("not found")
        || lower.contains("usb")
        || lower.contains("connection refused")
        || lower.contains("disconnected")
    {
        WalletError::HardwareUnsupported(format!(
            "Trezor not ready — unlock, check USB. On Linux: Settings → h or trezor.io/guides/trezorctl/udev-rules: {msg}"
        ))
    } else if lower.contains("not unique") || lower.contains("multiple") {
        WalletError::HardwareUnsupported(
            "Multiple Trezors detected — unplug extras (or disable debug USB)".into(),
        )
    } else if lower.contains("cancel") || lower.contains("reject") || lower.contains("denied") {
        WalletError::SigningFailed("rejected on Trezor".into())
    } else if lower.contains("pin") {
        WalletError::HardwareUnsupported(format!("Trezor PIN: {msg}"))
    } else {
        WalletError::SigningFailed(format!("Trezor: {msg}"))
    }
}

/// Parse `m/44'/60'/0'/0/0` into Trezor `address_n` (hardened bit set).
pub(crate) fn path_to_address_n(path: &str) -> Result<Vec<u32>, WalletError> {
    let p = path.trim();
    if p.is_empty() {
        return Err(WalletError::InvalidTransaction(
            "empty Trezor derivation path".into(),
        ));
    }
    let body = p.strip_prefix("m/").unwrap_or(p);
    let mut out = Vec::new();
    for part in body.split('/') {
        if part.is_empty() {
            return Err(WalletError::InvalidTransaction(format!(
                "invalid Trezor path segment in {path}"
            )));
        }
        let hardened = part.ends_with('\'') || part.ends_with('h') || part.ends_with('H');
        let num_str = part.trim_end_matches(['\'', 'h', 'H']);
        let n: u32 = num_str.parse().map_err(|_| {
            WalletError::InvalidTransaction(format!("invalid Trezor path segment: {part}"))
        })?;
        out.push(if hardened { n | 0x8000_0000 } else { n });
    }
    if out.is_empty() {
        return Err(WalletError::InvalidTransaction(
            "empty Trezor derivation path".into(),
        ));
    }
    Ok(out)
}

/// Open the unique non-debug Trezor and run Initialize (PIN matrix / passphrase aware).
///
/// Sends [`protos::EndSession`] first (best-effort) so a prior passphrase on the
/// same USB plug does not stick — standard vs hidden wallets must not share a
/// cached session when the user switches F3 accounts.
pub(crate) fn open_initialized(
    passphrase: Option<&SecretString>,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<Trezor, WalletError> {
    open_session(passphrase, ui, None).map(|(device, _)| device)
}

/// Like [`open_initialized`], but resumes the device session from the last
/// sign for `address` so the passphrase is asked once per wallet, not per sign.
///
/// Caller must still verify the device address matches `address`; on mismatch
/// call [`TrezorUiBridge::clear_session`].
///
/// Also returns the `Features` from Initialize: `Trezor::initialize` does not
/// cache them on the client, so `Trezor::features()` stays `None`.
pub(crate) fn open_for_account(
    passphrase: Option<&SecretString>,
    ui: Option<&Arc<TrezorUiBridge>>,
    address: &str,
) -> Result<(Trezor, protos::Features), WalletError> {
    open_session(passphrase, ui, Some(address))
}

fn open_session(
    passphrase: Option<&SecretString>,
    ui: Option<&Arc<TrezorUiBridge>>,
    address: Option<&str>,
) -> Result<(Trezor, protos::Features), WalletError> {
    wait_for_usb_device(ui)?;
    let mut device = trezor_client::unique(false).map_err(map_trezor)?;
    let resume = match (ui, address) {
        (Some(bridge), Some(addr)) => bridge.cached_session(addr),
        _ => None,
    };
    if resume.is_none() {
        // Clear passphrase cache from a previous Vaughan connect on this plug.
        let _ = device.call_raw(protos::EndSession::new());
    }
    let init = device.initialize(resume).map_err(map_trezor)?;
    let features = handle_host_ui(init, passphrase, ui, false)?;
    if let (Some(bridge), Some(addr)) = (ui, address) {
        let id = features.session_id();
        if !id.is_empty() {
            bridge.store_session(addr, id.to_vec());
        }
    }
    Ok((device, features))
}

/// How long a sign waits for the Trezor to be plugged in (under the 120 s dApp timeout).
const DEVICE_WAIT: Duration = Duration::from_secs(90);
const DEVICE_POLL: Duration = Duration::from_millis(500);

/// With a host UI, wait (showing "connect your Trezor") until one is on USB.
///
/// Polls WebUSB only: [`trezor_client::unique`] also pings the UDP emulator and
/// logs a warning each call. Without a UI, returns at once and lets `unique`
/// report the usual not-found error.
fn wait_for_usb_device(ui: Option<&Arc<TrezorUiBridge>>) -> Result<(), WalletError> {
    use trezor_client::transport::webusb::WebUsbTransport;

    let present = || {
        WebUsbTransport::find_devices(false)
            .map(|d| !d.is_empty())
            .unwrap_or(false)
    };
    let Some(bridge) = ui else {
        return Ok(());
    };
    if present() {
        return Ok(());
    }
    // Drop a stale Esc from an earlier session so the wait is not cancelled at once.
    let _ = bridge.take_abort();
    bridge.set_awaiting_device(true);
    let start = Instant::now();
    let result = loop {
        if bridge.take_abort() {
            break Err(WalletError::SigningFailed(
                "Trezor: cancelled on host — device not connected".into(),
            ));
        }
        if present() {
            break Ok(());
        }
        if start.elapsed() >= DEVICE_WAIT {
            break Err(WalletError::HardwareUnsupported(
                "Trezor not connected — plug it in and try again".into(),
            ));
        }
        std::thread::sleep(DEVICE_POLL);
    };
    bridge.set_awaiting_device(false);
    result
}

/// Button auto-ack; Trezor One PIN via bridge; passphrase on-device or session secret.
///
/// `track_button`: when true, set [`TrezorUiBridge::set_awaiting_button`] around
/// ButtonRequest (tx / message confirm). When false (Initialize / GetAddress),
/// skip that flag so the TUI does not flash "confirm on Trezor" before host
/// passphrase entry.
pub(crate) fn handle_host_ui<T, R: TrezorMessage>(
    resp: TrezorResponse<'_, T, R>,
    passphrase: Option<&SecretString>,
    ui: Option<&Arc<TrezorUiBridge>>,
    track_button: bool,
) -> Result<T, WalletError> {
    match resp {
        TrezorResponse::Ok(res) => Ok(res),
        TrezorResponse::Failure(f) => Err(map_trezor(format!("{f:?}"))),
        TrezorResponse::ButtonRequest(req) => {
            // Esc on the confirm overlay before ButtonAck — Cancel so firmware
            // never enters the on-device confirm screen.
            if ui.is_some_and(|b| b.take_abort()) {
                send_cancel(req.client);
                return Err(WalletError::SigningFailed(
                    "Trezor: cancelled on host — confirm aborted".into(),
                ));
            }
            if track_button {
                if let Some(bridge) = ui {
                    bridge.set_awaiting_button(true);
                }
            }
            let next = req.ack().map_err(map_trezor);
            if track_button {
                if let Some(bridge) = ui {
                    bridge.set_awaiting_button(false);
                }
            }
            handle_host_ui(next?, passphrase, ui, track_button)
        }
        TrezorResponse::PinMatrixRequest(req) => {
            let Some(bridge) = ui else {
                return Err(WalletError::HardwareUnsupported(
                    "Trezor One needs a host PIN matrix — open from the TUI (c Hardware)".into(),
                ));
            };
            match bridge.request_pin() {
                Ok(pin) if !pin.is_empty() => handle_host_ui(
                    req.ack_pin(pin).map_err(map_trezor)?,
                    passphrase,
                    ui,
                    track_button,
                ),
                Ok(_) => {
                    // Empty submit — treat as host cancel; tell the device to leave PIN.
                    send_cancel(req.client);
                    Err(WalletError::HardwareUnsupported(
                        "Trezor PIN entry cancelled".into(),
                    ))
                }
                Err(e) => {
                    // Esc / interrupt — Cancel so the device clears its PIN screen.
                    send_cancel(req.client);
                    Err(e)
                }
            }
        }
        TrezorResponse::PassphraseRequest(req) => {
            if req.on_device() {
                handle_host_ui(
                    req.ack(true).map_err(map_trezor)?,
                    passphrase,
                    ui,
                    track_button,
                )
            } else if let Some(secret) = passphrase {
                let pass = secret.expose_secret().clone();
                handle_host_ui(
                    req.ack_passphrase(pass).map_err(map_trezor)?,
                    passphrase,
                    ui,
                    track_button,
                )
            } else {
                // Trezor One: passphrase is always entered on the host.
                let Some(bridge) = ui else {
                    return Err(WalletError::HardwareUnsupported(
                        "Trezor passphrase required — host UI bridge missing".into(),
                    ));
                };
                match bridge.request_passphrase() {
                    Ok(secret) => {
                        let pass = secret.expose_secret().clone();
                        handle_host_ui(
                            req.ack_passphrase(pass).map_err(map_trezor)?,
                            passphrase,
                            ui,
                            track_button,
                        )
                    }
                    Err(e) => {
                        // Esc / interrupt — Cancel so firmware leaves the passphrase screen.
                        send_cancel(req.client);
                        Err(e)
                    }
                }
            }
        }
    }
}

/// Host Esc / abort: send protobuf `Cancel` so firmware leaves PIN / confirm.
fn send_cancel(client: &mut Trezor) {
    let _ = client.call_raw(protos::Cancel::new());
}

/// Best-effort Cancel on a fresh USB handle (confirm-on-device Esc).
///
/// Often no-ops while the signing thread holds the device; PIN Esc still clears
/// via [`send_cancel`] on that same session.
pub fn best_effort_host_cancel() {
    std::thread::spawn(|| {
        let Ok(mut device) = trezor_client::unique(false) else {
            return;
        };
        let _ = device.initialize(None);
        let _ = device.call_raw(protos::Cancel::new());
    });
}

/// Address at path (uses host UI for Trezor One PIN — not crate `handle_interaction`).
pub(crate) fn ethereum_address(
    device: &mut Trezor,
    path: &str,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<String, WalletError> {
    let address_n = path_to_address_n(path)?;
    let mut req = protos::EthereumGetAddress::new();
    req.address_n = address_n;
    let resp = device
        .call(
            req,
            Box::new(|_, m: protos::EthereumAddress| Ok(m.address().to_string())),
        )
        .map_err(map_trezor)?;
    handle_host_ui(resp, None, ui, false)
}

pub(crate) fn ethereum_personal_sign(
    device: &mut Trezor,
    path: &str,
    message: Vec<u8>,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<String, WalletError> {
    let address_n = path_to_address_n(path)?;
    let mut req = protos::EthereumSignMessage::new();
    req.address_n = address_n;
    req.set_message(message);
    let resp = device
        .call(
            req,
            Box::new(|_, m: protos::EthereumMessageSignature| {
                let signature = m.signature();
                if signature.len() != 65 {
                    return Err(trezor_client::Error::MalformedSignature);
                }
                let r = signature[0..32].try_into().unwrap();
                let s = signature[32..64].try_into().unwrap();
                let v = signature[64] as u64;
                Ok(TrezorSignature { r, s, v })
            }),
        )
        .map_err(map_trezor)?;
    let sig = handle_host_ui(resp, None, ui, true)?;
    Ok(signature_hex_personal(&sig))
}

/// EIP-712 via `EthereumSignTypedHash` — the only typed-data mode on Trezor One.
///
/// The device shows the domain and message hashes (not decoded fields); the
/// host approve card must show the same pair. Returns the signature hex and the
/// address the device reports having signed with.
pub(crate) fn ethereum_sign_typed_hash(
    device: &mut Trezor,
    path: &str,
    hashes: &Eip712Hashes,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<(String, String), WalletError> {
    let address_n = path_to_address_n(path)?;
    let mut req = protos::EthereumSignTypedHash::new();
    req.address_n = address_n;
    req.set_domain_separator_hash(hashes.domain.to_vec());
    if let Some(message) = hashes.message {
        req.set_message_hash(message.to_vec());
    }
    let resp = device
        .call(
            req,
            Box::new(|_, m: protos::EthereumTypedDataSignature| {
                let signature = m.signature();
                if signature.len() != 65 {
                    return Err(trezor_client::Error::MalformedSignature);
                }
                let r = signature[0..32].try_into().unwrap();
                let s = signature[32..64].try_into().unwrap();
                let v = signature[64] as u64;
                Ok((TrezorSignature { r, s, v }, m.address().to_string()))
            }),
        )
        .map_err(map_trezor)?;
    let (sig, address) = handle_host_ui(resp, None, ui, true)?;
    Ok((signature_hex_personal(&sig), address))
}

/// Trezor One: host PIN matrix, no EIP-712 field streaming.
pub(crate) fn is_trezor_one(device: &Trezor, features: &protos::Features) -> bool {
    reports_trezor_one(
        device.model(),
        Some(features.model()),
        Some(features.internal_model()),
    )
}

/// Trezor One firmware >= 1.8 enumerates on the same WebUSB id as Model T,
/// so `trezor_client::Model` alone reads it as `Trezor`. Prefer the model the
/// device reports in `Features` (`"1"` / `T1B1`); fall back to the USB id.
fn reports_trezor_one(
    usb_model: trezor_client::Model,
    model: Option<&str>,
    internal_model: Option<&str>,
) -> bool {
    match (
        model.filter(|m| !m.is_empty()),
        internal_model.filter(|m| !m.is_empty()),
    ) {
        (None, None) => matches!(
            usb_model,
            trezor_client::Model::TrezorLegacy | trezor_client::Model::TrezorBootloader
        ),
        (model, internal) => model == Some("1") || internal == Some("T1B1"),
    }
}

/// EIP-712 via `EthereumSignTypedData` streaming (Model T / Safe): the device
/// pulls struct layouts and field values and shows them before signing.
/// Returns the signature hex and the address the device reports.
pub(crate) fn ethereum_sign_typed_data(
    device: &mut Trezor,
    path: &str,
    payload: &serde_json::Value,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<(String, String), WalletError> {
    use protos::MessageType as Mt;

    let stream = TypedDataStream::new(payload)?;
    let mut req = protos::EthereumSignTypedData::new();
    req.address_n = path_to_address_n(path)?;
    req.set_primary_type(stream.primary_type().to_string());
    req.set_metamask_v4_compat(true);
    let bad_reply = |e: &dyn std::fmt::Display| map_trezor(format!("malformed reply: {e}"));

    let mut resp = device.call_raw(req).map_err(map_trezor)?;
    loop {
        resp = match resp.message_type() {
            Mt::MessageType_EthereumTypedDataStructRequest => {
                let r = resp
                    .into_message::<protos::EthereumTypedDataStructRequest>()
                    .map_err(|e| bad_reply(&e))?;
                let members = stream
                    .struct_members(r.name())
                    .inspect_err(|_| send_cancel(device))?;
                let mut ack = protos::EthereumTypedDataStructAck::new();
                ack.members = members;
                device.call_raw(ack).map_err(map_trezor)?
            }
            Mt::MessageType_EthereumTypedDataValueRequest => {
                let r = resp
                    .into_message::<protos::EthereumTypedDataValueRequest>()
                    .map_err(|e| bad_reply(&e))?;
                let value = stream
                    .value_at(&r.member_path)
                    .inspect_err(|_| send_cancel(device))?;
                let mut ack = protos::EthereumTypedDataValueAck::new();
                ack.set_value(value);
                device.call_raw(ack).map_err(map_trezor)?
            }
            Mt::MessageType_ButtonRequest => {
                if ui.is_some_and(|b| b.take_abort()) {
                    send_cancel(device);
                    return Err(WalletError::SigningFailed(
                        "Trezor: cancelled on host — confirm aborted".into(),
                    ));
                }
                if let Some(bridge) = ui {
                    bridge.set_awaiting_button(true);
                }
                let next = device.call_raw(protos::ButtonAck::new());
                if let Some(bridge) = ui {
                    bridge.set_awaiting_button(false);
                }
                next.map_err(map_trezor)?
            }
            Mt::MessageType_EthereumTypedDataSignature => {
                let m = resp
                    .into_message::<protos::EthereumTypedDataSignature>()
                    .map_err(|e| bad_reply(&e))?;
                let signature = m.signature();
                if signature.len() != 65 {
                    return Err(map_trezor("malformed EIP-712 signature"));
                }
                let sig = TrezorSignature {
                    r: signature[0..32].try_into().unwrap(),
                    s: signature[32..64].try_into().unwrap(),
                    v: signature[64] as u64,
                };
                return Ok((signature_hex_personal(&sig), m.address().to_string()));
            }
            Mt::MessageType_Failure => {
                let f = resp
                    .into_message::<protos::Failure>()
                    .map_err(|e| bad_reply(&e))?;
                if f.code() == protos::failure::FailureType::Failure_UnexpectedMessage {
                    return Err(WalletError::HardwareUnsupported(
                        "Trezor refused full EIP-712 (Trezor One signs hashes only; \
                         Model T / Safe: update firmware in Trezor Suite)"
                            .into(),
                    ));
                }
                return Err(map_trezor(format!("{:?}: {}", f.code(), f.message())));
            }
            other => {
                send_cancel(device);
                return Err(WalletError::SigningFailed(format!(
                    "Trezor: unexpected {other:?} during EIP-712"
                )));
            }
        };
    }
}

pub(crate) fn ethereum_sign_prepared_tx(
    device: &mut Trezor,
    path: &str,
    evm_tx: &EvmTransaction,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<Vec<u8>, WalletError> {
    let address_n = path_to_address_n(path)?;
    let from = Address::from_str(evm_tx.from.trim()).map_err(|_| {
        WalletError::InvalidTransaction(format!("invalid address: {}", evm_tx.from))
    })?;
    let to_addr = Address::from_str(evm_tx.to.trim())
        .map_err(|_| WalletError::InvalidTransaction(format!("invalid address: {}", evm_tx.to)))?;
    let value = U256::from_str(&evm_tx.value)
        .map_err(|_| WalletError::InvalidAmount(format!("Invalid wei value: {}", evm_tx.value)))?;
    let nonce = evm_tx.nonce.ok_or_else(|| {
        WalletError::InvalidTransaction("nonce required before Trezor envelope sign".into())
    })?;
    let gas_limit = evm_tx.gas_limit.ok_or_else(|| {
        WalletError::InvalidTransaction("gas_limit required before Trezor envelope sign".into())
    })?;
    let data = match evm_tx.data.as_deref() {
        Some(hex_data) => hex::decode(hex_data.trim_start_matches("0x"))
            .map_err(|_| WalletError::InvalidTransaction("Invalid hex data".into()))?,
        None => Vec::new(),
    };
    let is_create = to_addr.is_zero() && !data.is_empty();
    let to_str = if is_create {
        String::new()
    } else {
        to_addr.to_checksum(None)
    };
    let chain_id = evm_tx.chain_id;
    let tx_kind = if is_create {
        TxKind::Create
    } else {
        TxKind::Call(to_addr)
    };

    // Trezor One (legacy) has no reliable EIP-1559 clear-signing — use type-0.
    // Model T / Safe can sign EIP-1559 when fees are present.
    let eip1559_ok = !matches!(
        device.model(),
        trezor_client::Model::TrezorLegacy | trezor_client::Model::TrezorBootloader
    );
    let want_eip1559 =
        eip1559_ok && evm_tx.max_fee_per_gas.is_some() && evm_tx.max_priority_fee_per_gas.is_some();

    if want_eip1559 {
        let max_fee = evm_tx.max_fee_per_gas.as_deref().unwrap();
        let prio = evm_tx.max_priority_fee_per_gas.as_deref().unwrap();
        let max_fee_u = U256::from_str(max_fee)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid max fee: {max_fee}")))?;
        let prio_u = U256::from_str(prio)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid priority fee: {prio}")))?;
        match sign_eip1559(
            device,
            address_n.clone(),
            u256_minimal_be(U256::from(nonce)),
            u256_minimal_be(U256::from(gas_limit)),
            to_str.clone(),
            u256_minimal_be(value),
            data.clone(),
            chain_id,
            u256_minimal_be(max_fee_u),
            u256_minimal_be(prio_u),
            ui,
        ) {
            Ok(sig) => {
                match encode_eip1559(
                    chain_id,
                    nonce,
                    prio_u,
                    max_fee_u,
                    gas_limit,
                    tx_kind,
                    value,
                    data.clone().into(),
                    &sig,
                    from,
                ) {
                    Ok(raw) => return Ok(raw),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Trezor EIP-1559 envelope recover failed; re-signing as legacy"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Trezor EIP-1559 sign failed; falling back to legacy tx"
                );
            }
        }
    }

    let gas_price = evm_tx
        .gas_price
        .as_deref()
        .or(evm_tx.max_fee_per_gas.as_deref())
        .ok_or_else(|| {
            WalletError::InvalidTransaction(
                "gas_price or EIP-1559 fees required before Trezor sign".into(),
            )
        })?;
    let gas_price = U256::from_str(gas_price)
        .map_err(|_| WalletError::InvalidAmount(format!("Invalid gas price: {gas_price}")))?;
    let sig = sign_legacy(
        device,
        address_n,
        u256_minimal_be(U256::from(nonce)),
        u256_minimal_be(gas_price),
        u256_minimal_be(U256::from(gas_limit)),
        to_str,
        u256_minimal_be(value),
        data.clone(),
        chain_id,
        ui,
    )?;
    encode_legacy(
        chain_id,
        nonce,
        gas_price,
        gas_limit,
        tx_kind,
        value,
        data.into(),
        &sig,
        from,
    )
}

#[allow(clippy::too_many_arguments)]
fn sign_eip1559(
    device: &mut Trezor,
    path: Vec<u32>,
    nonce: Vec<u8>,
    gas_limit: Vec<u8>,
    to: String,
    value: Vec<u8>,
    mut data: Vec<u8>,
    chain_id: u64,
    max_gas_fee: Vec<u8>,
    max_priority_fee: Vec<u8>,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<TrezorSignature, WalletError> {
    use protos::ethereum_sign_tx_eip1559::EthereumAccessList;

    let mut req = protos::EthereumSignTxEIP1559::new();
    req.address_n = path;
    req.set_nonce(nonce);
    req.set_max_gas_fee(max_gas_fee);
    req.set_max_priority_fee(max_priority_fee);
    req.set_gas_limit(gas_limit);
    req.set_value(value);
    req.set_chain_id(chain_id);
    req.set_to(to);
    req.access_list = Vec::<EthereumAccessList>::new();
    req.set_data_length(data.len() as u32);
    req.set_data_initial_chunk(data.splice(..std::cmp::min(1024, data.len()), []).collect());

    let mut resp = handle_host_ui(
        device
            .call(req, Box::new(|_, m: protos::EthereumTxRequest| Ok(m)))
            .map_err(map_trezor)?,
        None,
        ui,
        true,
    )?;

    while resp.data_length() > 0 {
        let mut ack = protos::EthereumTxAck::new();
        ack.set_data_chunk(data.splice(..std::cmp::min(1024, data.len()), []).collect());
        resp = handle_host_ui(
            device
                .call(ack, Box::new(|_, m: protos::EthereumTxRequest| Ok(m)))
                .map_err(map_trezor)?,
            None,
            ui,
            true,
        )?;
    }
    convert_signature(&resp, None)
}

#[allow(clippy::too_many_arguments)]
fn sign_legacy(
    device: &mut Trezor,
    path: Vec<u32>,
    nonce: Vec<u8>,
    gas_price: Vec<u8>,
    gas_limit: Vec<u8>,
    to: String,
    value: Vec<u8>,
    mut data: Vec<u8>,
    chain_id: u64,
    ui: Option<&Arc<TrezorUiBridge>>,
) -> Result<TrezorSignature, WalletError> {
    let mut req = protos::EthereumSignTx::new();
    req.address_n = path;
    req.set_nonce(nonce);
    req.set_gas_price(gas_price);
    req.set_gas_limit(gas_limit);
    req.set_value(value);
    req.set_chain_id(chain_id);
    req.set_to(to);
    req.set_data_length(data.len() as u32);
    req.set_data_initial_chunk(data.splice(..std::cmp::min(1024, data.len()), []).collect());

    let mut resp = handle_host_ui(
        device
            .call(req, Box::new(|_, m: protos::EthereumTxRequest| Ok(m)))
            .map_err(map_trezor)?,
        None,
        ui,
        true,
    )?;

    while resp.data_length() > 0 {
        let mut ack = protos::EthereumTxAck::new();
        ack.set_data_chunk(data.splice(..std::cmp::min(1024, data.len()), []).collect());
        resp = handle_host_ui(
            device
                .call(ack, Box::new(|_, m: protos::EthereumTxRequest| Ok(m)))
                .map_err(map_trezor)?,
            None,
            ui,
            true,
        )?;
    }
    // Legacy firmware may return raw recid (0/1) or full EIP-155 v.
    convert_signature(&resp, Some(chain_id))
}

fn convert_signature(
    resp: &protos::EthereumTxRequest,
    chain_id: Option<u64>,
) -> Result<TrezorSignature, WalletError> {
    let mut v = resp.signature_v() as u64;
    if let Some(chain_id) = chain_id {
        if v <= 1 {
            v = v + 2 * chain_id + 35;
        }
    }
    let r = sig_component_32(resp.signature_r())?;
    let s = sig_component_32(resp.signature_s())?;
    Ok(TrezorSignature { r, s, v })
}

/// Left-pad Trezor `r`/`s` to 32 bytes (device may omit leading zeros).
fn sig_component_32(bytes: &[u8]) -> Result<[u8; 32], WalletError> {
    if bytes.len() > 32 {
        return Err(WalletError::SigningFailed(
            "malformed Trezor signature component".into(),
        ));
    }
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(bytes);
    Ok(out)
}

fn u256_minimal_be(v: U256) -> Vec<u8> {
    // Ethereum RLP encodes integer 0 as the empty byte string (`0x80`), not
    // `0x00`. Trezor hashes the raw field bytes we send — a leading `0x00` for
    // nonce 0 produces a different digest than Alloy's sighash and the recovered
    // signer will not match the watch address.
    if v.is_zero() {
        return Vec::new();
    }
    let full = v.to_be_bytes::<32>();
    let start = full.iter().position(|&b| b != 0).unwrap_or(0);
    full[start..].to_vec()
}

fn signature_hex_personal(sig: &TrezorSignature) -> String {
    let mut v = sig.v;
    if v <= 1 {
        v += 27;
    }
    format!(
        "0x{}{}{:02x}",
        hex::encode(sig.r),
        hex::encode(sig.s),
        (v & 0xff) as u8
    )
}

fn y_parity(sig: &TrezorSignature, chain_id: u64) -> Result<bool, WalletError> {
    let v = sig.v;
    if v <= 1 {
        return Ok(v == 1);
    }
    if v == 27 || v == 28 {
        return Ok(v == 28);
    }
    if v >= 35 {
        let base = 35u64.saturating_add(2u64.saturating_mul(chain_id));
        if let Some(y) = v.checked_sub(base) {
            if y <= 1 {
                return Ok(y == 1);
            }
        }
    }
    Err(WalletError::SigningFailed(format!(
        "unexpected Trezor signature v={v}"
    )))
}

fn alloy_sig(sig: &TrezorSignature, chain_id: u64) -> Result<AlloySignature, WalletError> {
    let parity = y_parity(sig, chain_id)?;
    Ok(AlloySignature::from_bytes_and_parity(
        &[sig.r, sig.s].concat(),
        parity,
    ))
}

/// Prefer the reported y-parity; if recovery misses `expected`, try the flip
/// (covers ambiguous v encodings without changing the signed digest).
fn alloy_sig_matching(
    sig: &TrezorSignature,
    chain_id: u64,
    sighash: alloy::primitives::B256,
    expected_from: Address,
) -> Result<AlloySignature, WalletError> {
    let reported = y_parity(sig, chain_id).ok();
    let mut tried = Vec::with_capacity(2);
    for parity in [reported, Some(false), Some(true)].into_iter().flatten() {
        if tried.contains(&parity) {
            continue;
        }
        tried.push(parity);
        let alloy_sig = AlloySignature::from_bytes_and_parity(&[sig.r, sig.s].concat(), parity);
        if alloy_sig.recover_address_from_prehash(&sighash).ok() == Some(expected_from) {
            return Ok(alloy_sig);
        }
    }
    let alloy_sig = alloy_sig(sig, chain_id)?;
    let recovered = alloy_sig
        .recover_address_from_prehash(&sighash)
        .map_err(|e| WalletError::SigningFailed(format!("Trezor recover failed: {e}")))?;
    Err(WalletError::SigningFailed(format!(
        "Trezor signature recovers to {recovered}, expected {expected_from}"
    )))
}

#[allow(clippy::too_many_arguments)]
fn encode_eip1559(
    chain_id: u64,
    nonce: u64,
    max_priority: U256,
    max_fee: U256,
    gas_limit: u64,
    to: TxKind,
    value: U256,
    input: Bytes,
    sig: &TrezorSignature,
    expected_from: Address,
) -> Result<Vec<u8>, WalletError> {
    let tx = TxEip1559 {
        chain_id,
        nonce,
        gas_limit,
        max_fee_per_gas: max_fee.to::<u128>(),
        max_priority_fee_per_gas: max_priority.to::<u128>(),
        to,
        value,
        access_list: Default::default(),
        input,
    };
    let sighash = tx.signature_hash();
    let alloy_sig = alloy_sig_matching(sig, chain_id, sighash, expected_from)?;
    let signed = tx.into_signed(alloy_sig);
    let envelope: TxEnvelope = signed.into();
    Ok(envelope.encoded_2718())
}

#[allow(clippy::too_many_arguments)]
fn encode_legacy(
    chain_id: u64,
    nonce: u64,
    gas_price: U256,
    gas_limit: u64,
    to: TxKind,
    value: U256,
    input: Bytes,
    sig: &TrezorSignature,
    expected_from: Address,
) -> Result<Vec<u8>, WalletError> {
    let tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: gas_price.to::<u128>(),
        gas_limit,
        to,
        value,
        input,
    };
    let sighash = tx.signature_hash();
    let alloy_sig = alloy_sig_matching(sig, chain_id, sighash, expected_from)?;
    let signed = tx.into_signed(alloy_sig);
    let envelope: TxEnvelope = signed.into();
    Ok(envelope.encoded_2718())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hardened_bip44_path() {
        let n = path_to_address_n("m/44'/60'/0'/0/0").unwrap();
        assert_eq!(n.len(), 5);
        assert_eq!(n[0], 44 | 0x8000_0000);
        assert_eq!(n[1], 60 | 0x8000_0000);
        assert_eq!(n[2], 0x8000_0000);
        assert_eq!(n[3], 0);
        assert_eq!(n[4], 0);
    }

    #[test]
    fn u256_minimal_strips_zeros() {
        assert_eq!(u256_minimal_be(U256::from(0u64)), Vec::<u8>::new());
        assert_eq!(u256_minimal_be(U256::from(0x100u64)), vec![0x01, 0x00]);
        assert_eq!(u256_minimal_be(U256::from(1u64)), vec![0x01]);
    }

    #[test]
    fn convert_signature_eip1559_keeps_y_parity() {
        // EIP-1559 firmware returns v ∈ {0,1}; do not apply EIP-155 (legacy-only).
        let mut resp = protos::EthereumTxRequest::new();
        resp.set_signature_v(1);
        resp.set_signature_r(vec![1u8; 32]);
        resp.set_signature_s(vec![2u8; 32]);
        let sig = convert_signature(&resp, None).unwrap();
        assert_eq!(sig.v, 1);
        assert!(y_parity(&sig, 943).unwrap());
    }

    #[test]
    fn convert_signature_legacy_applies_eip155_for_recid() {
        let mut resp = protos::EthereumTxRequest::new();
        resp.set_signature_v(0);
        resp.set_signature_r(vec![1u8; 32]);
        resp.set_signature_s(vec![2u8; 32]);
        let sig = convert_signature(&resp, Some(943)).unwrap();
        assert_eq!(sig.v, 2 * 943 + 35);
        assert!(!y_parity(&sig, 943).unwrap());
    }

    #[test]
    fn trezor_one_on_webusb_detected_from_features() {
        use trezor_client::Model;
        // Modern Trezor One: WebUSB id reads as `Trezor`, features say "1".
        assert!(reports_trezor_one(Model::Trezor, Some("1"), Some("")));
        assert!(reports_trezor_one(Model::Trezor, None, Some("T1B1")));
        assert!(!reports_trezor_one(Model::Trezor, Some("T"), Some("T2T1")));
        assert!(!reports_trezor_one(
            Model::Trezor,
            Some("Safe 3"),
            Some("T2B1")
        ));
        // No features: fall back to the USB id.
        assert!(reports_trezor_one(Model::TrezorLegacy, None, None));
        assert!(!reports_trezor_one(Model::Trezor, Some(""), None));
    }
}
