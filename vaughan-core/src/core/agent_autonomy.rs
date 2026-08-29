//! Agent autonomy tiers for the adviser (`default`) profile.
//!
//! **Advisor** (default): every `eth_requestAccounts` shows the Connect approval card.
//! **Operator**: auto-grants connect for origins on the trusted-dApp + Ag catalog
//! allowlist (VB / aggregator tour). Signing and `propose_*` still require human
//! approval on non-sentient profiles.

use serde::{Deserialize, Serialize};
use url::Url;

use crate::core::aggregator::AGG_VENUES;
use crate::core::persistence::{trusted_dapp_allow_hosts, TrustedDapp};

/// How much wallet-connect autonomy MCP / VB agents receive before signing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentAutonomyTier {
    /// Manual Connect approval for every new site (today's default).
    #[default]
    Advisor,
    /// Auto-connect on trusted dApp + Ag catalog hosts; never auto-sign.
    Operator,
}

impl AgentAutonomyTier {
    /// Short label for the F1 network strip.
    pub fn chrome_label(self) -> Option<&'static str> {
        match self {
            Self::Advisor => None,
            Self::Operator => Some("Op"),
        }
    }

    /// Parse CLI / config strings (`advisor`, `operator`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "advisor" | "adviser" => Some(Self::Advisor),
            "operator" | "op" => Some(Self::Operator),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advisor => "advisor",
            Self::Operator => "operator",
        }
    }
}

/// Host suffixes eligible for Operator-tier auto-connect.
pub fn operator_connect_allow_suffixes(trusted_dapps: &[TrustedDapp]) -> Vec<String> {
    let mut suffixes = trusted_dapp_allow_hosts(trusted_dapps);
    for venue in AGG_VENUES {
        if let Some(raw) = venue.web_url() {
            if let Ok(u) = Url::parse(raw) {
                if let Some(host) = u.host_str() {
                    push_host_suffix(&mut suffixes, host);
                }
            }
        }
        if let Some(raw) = venue.web_url_pls_hex() {
            if let Ok(u) = Url::parse(&raw) {
                if let Some(host) = u.host_str() {
                    push_host_suffix(&mut suffixes, host);
                }
            }
        }
    }
    suffixes
}

fn push_host_suffix(out: &mut Vec<String>, host: &str) {
    let h = host.trim().trim_start_matches('.').to_ascii_lowercase();
    if h.is_empty() || out.iter().any(|x| x == &h) {
        return;
    }
    out.push(h.clone());
    let parts: Vec<&str> = h.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 {
        let parent = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        if !out.iter().any(|x| x == &parent) {
            out.push(parent);
        }
    }
}

fn host_matches_suffix(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

/// Whether Operator tier may auto-grant `eth_requestAccounts` for `site`.
///
/// `site` is the connect grant key (page origin from VB extension path).
pub fn operator_connect_allowed(site: &str, trusted_dapps: &[TrustedDapp]) -> bool {
    let site = site.trim();
    if site.is_empty() {
        return false;
    }
    let u = match Url::parse(site) {
        Ok(u) => u,
        Err(_) => return false,
    };
    match u.scheme() {
        "https" => {}
        "http" => {
            let h = u.host_str().unwrap_or("");
            if h != "localhost" && h != "127.0.0.1" {
                return false;
            }
        }
        _ => return false,
    }
    let host = match u.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };

    let suffixes = operator_connect_allow_suffixes(trusted_dapps);
    suffixes.iter().any(|suf| host_matches_suffix(&host, suf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::persistence::default_trusted_dapps;

    #[test]
    fn default_tier_is_advisor() {
        assert_eq!(AgentAutonomyTier::default(), AgentAutonomyTier::Advisor);
        assert!(AgentAutonomyTier::Advisor.chrome_label().is_none());
        assert_eq!(AgentAutonomyTier::Operator.chrome_label(), Some("Op"));
    }

    #[test]
    fn operator_allows_switch_and_9mm_catalog_hosts() {
        let dapps = default_trusted_dapps();
        assert!(operator_connect_allowed("https://www.switch.win", &dapps));
        assert!(operator_connect_allowed("https://beta.switch.win", &dapps));
        assert!(operator_connect_allowed("https://9x.9mm.pro", &dapps));
        assert!(operator_connect_allowed("https://app.piteas.io", &dapps));
    }

    #[test]
    fn operator_rejects_unknown_origin() {
        let dapps = default_trusted_dapps();
        assert!(!operator_connect_allowed(
            "https://evil-phish.example",
            &dapps
        ));
        assert!(!operator_connect_allowed(
            "chrome-extension://deadbeef",
            &dapps
        ));
    }

    #[test]
    fn parse_tier_aliases() {
        assert_eq!(
            AgentAutonomyTier::parse("operator"),
            Some(AgentAutonomyTier::Operator)
        );
        assert_eq!(
            AgentAutonomyTier::parse("adviser"),
            Some(AgentAutonomyTier::Advisor)
        );
        assert!(AgentAutonomyTier::parse("sentient").is_none());
    }
}
