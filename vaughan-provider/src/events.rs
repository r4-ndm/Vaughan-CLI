//! EIP-1193 event push to connected clients (FR-2.2).
//!
//! The host (TUI) publishes [`ProviderEvent`]s through an [`EventBus`]; the
//! server relays each event to every connected client as a JSON-RPC 2.0
//! **notification** — `{"jsonrpc":"2.0","method":"accountsChanged","params":[...]}`
//! — which is the wire shape EIP-1193 clients subscribe to. Clients that
//! joined before an event miss it (broadcast only retains recent events);
//! that is acceptable for account/chain changes, which clients re-query on
//! reconnect.

use tokio::sync::broadcast;

/// EIP-1193 provider events pushed to connected clients.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    /// `accountsChanged`: payload is the new account list (`[]` when locked).
    AccountsChanged(Vec<String>),
    /// `chainChanged`: payload is the chain id as a `0x` hex string.
    ChainChanged(String),
}

impl ProviderEvent {
    /// The EIP-1193 event name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::AccountsChanged(_) => "accountsChanged",
            Self::ChainChanged(_) => "chainChanged",
        }
    }

    /// The event payload (the `params` member of the notification).
    pub fn params(&self) -> serde_json::Value {
        match self {
            Self::AccountsChanged(accounts) => serde_json::json!(accounts),
            Self::ChainChanged(chain_id) => serde_json::Value::String(chain_id.clone()),
        }
    }

    /// The wire form: a JSON-RPC 2.0 notification (no `id`).
    pub fn to_notification(&self) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": self.method(),
            "params": self.params(),
        })
        .to_string()
    }
}

/// How many recent events a slow client may still receive after lagging.
const EVENT_CAPACITY: usize = 64;

/// Cloneable handle the host uses to publish provider events.
///
/// The server holds a reference and subscribes per connection; the host
/// keeps a handle and calls [`EventBus::publish`] on account/chain changes.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<String>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(EVENT_CAPACITY);
        Self { tx }
    }

    /// Publish an event to all connected clients.
    pub fn publish(&self, event: ProviderEvent) {
        let notification = event.to_notification();
        // Ignore the send error: with zero receivers there is nothing to do.
        let _ = self.tx.send(notification);
    }

    /// Subscribe to the event stream (used by the server per connection).
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wire_format() {
        let event = ProviderEvent::AccountsChanged(vec!["0xabc".into()]);
        assert_eq!(event.method(), "accountsChanged");
        let value: serde_json::Value = serde_json::from_str(&event.to_notification()).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "accountsChanged");
        assert_eq!(value["params"][0], "0xabc");
        assert!(value.get("id").is_none(), "events must be notifications");

        let chain = ProviderEvent::ChainChanged("0x171".into());
        let value: serde_json::Value = serde_json::from_str(&chain.to_notification()).unwrap();
        assert_eq!(value["method"], "chainChanged");
        assert_eq!(value["params"], "0x171");
    }

    #[tokio::test]
    async fn publish_reaches_subscriber() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(ProviderEvent::ChainChanged("0x1".into()));
        let notification = rx.recv().await.unwrap();
        assert!(notification.contains("\"method\":\"chainChanged\""));
    }
}
