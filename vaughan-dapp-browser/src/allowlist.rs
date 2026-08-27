//! Host allowlist for top-level navigation (multi-chain EVM dApps, not Pulse-only).

use url::Url;

/// HTTPS (or http://localhost) hosts the shell may open.
#[derive(Debug, Clone)]
pub struct Allowlist {
    /// Exact hosts or DNS suffixes (e.g. `pulsex.com` matches `app.pulsex.com`).
    suffixes: Vec<String>,
}

impl Allowlist {
    /// Build from the initial URL host plus optional extra suffixes.
    pub fn from_url_and_hosts(url: &str, extra: &[String]) -> Result<Self, String> {
        let parsed = Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
        let mut suffixes = Vec::new();
        if let Some(host) = parsed.host_str() {
            for s in expand_host_suffixes(host) {
                push_unique(&mut suffixes, s);
            }
        }
        for h in extra {
            let t = h.trim().to_ascii_lowercase();
            if !t.is_empty() {
                for s in expand_host_suffixes(&t) {
                    push_unique(&mut suffixes, s);
                }
            }
        }
        if suffixes.is_empty() {
            return Err("allowlist empty".into());
        }
        Ok(Self { suffixes })
    }

    /// Validate a navigation target.
    pub fn check_url(&self, raw: &str) -> Result<(), String> {
        if Self::is_ephemeral_url(raw) {
            return Ok(());
        }
        let u = Url::parse(raw).map_err(|e| format!("invalid url: {e}"))?;
        match u.scheme() {
            "https" => {}
            "http" => {
                let host = u.host_str().unwrap_or("");
                if host != "localhost" && host != "127.0.0.1" {
                    return Err("http only allowed for localhost".into());
                }
            }
            other => return Err(format!("unsupported scheme `{other}`")),
        }
        let host = u
            .host_str()
            .ok_or_else(|| "url missing host".to_string())?
            .to_ascii_lowercase();
        if self.host_allowed(&host) {
            Ok(())
        } else {
            Err(format!("host `{host}` not in allowlist"))
        }
    }

    /// Chrome internals / empty loads — do not treat as allowlist violations.
    pub fn is_ephemeral_url(raw: &str) -> bool {
        let t = raw.trim();
        t.is_empty()
            || t == "about:blank"
            || t.starts_with("chrome://")
            || t.starts_with("chrome-error://")
            || t.starts_with("chrome-extension://")
            || t.starts_with("devtools://")
            || t.starts_with("data:")
            || t.starts_with("blob:")
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.suffixes
            .iter()
            .any(|suf| host == suf || host.ends_with(&format!(".{suf}")))
    }

    /// Host suffixes written to the extension allowlist (navigation gate).
    pub fn suffixes(&self) -> &[String] {
        &self.suffixes
    }

    /// JSON blob for `allowlist.json` in the unpacked extension directory.
    pub fn to_extension_json(&self) -> String {
        serde_json::json!({ "suffixes": self.suffixes }).to_string()
    }
}

fn push_unique(out: &mut Vec<String>, s: String) {
    if !out.iter().any(|x| x == &s) {
        out.push(s);
    }
}

/// `app.pulsex.com` → `app.pulsex.com` + `pulsex.com` (parent for redirects).
fn expand_host_suffixes(host: &str) -> Vec<String> {
    let host = host.trim().trim_start_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Vec::new();
    }
    let mut out = vec![host.clone()];
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 {
        out.push(format!(
            "{}.{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_exact_and_subdomain() {
        let a = Allowlist::from_url_and_hosts("https://app.pulsex.com/", &[]).unwrap();
        a.check_url("https://app.pulsex.com/swap").unwrap();
        assert!(a.check_url("https://evil.com/").is_err());
    }

    #[test]
    fn allows_parent_domain_redirect() {
        let a = Allowlist::from_url_and_hosts("https://app.pulsex.com/", &[]).unwrap();
        a.check_url("https://pulsex.com/").unwrap();
        a.check_url("https://www.pulsex.com/").unwrap();
    }

    #[test]
    fn extra_suffix_multi_chain() {
        let a = Allowlist::from_url_and_hosts("https://app.pulsex.com/", &["uniswap.org".into()])
            .unwrap();
        a.check_url("https://app.uniswap.org/").unwrap();
    }

    #[test]
    fn localhost_http_ok() {
        let a = Allowlist::from_url_and_hosts("http://127.0.0.1:3000/", &[]).unwrap();
        a.check_url("http://127.0.0.1:3000/").unwrap();
    }

    #[test]
    fn ephemeral_urls() {
        assert!(Allowlist::is_ephemeral_url("about:blank"));
        assert!(Allowlist::is_ephemeral_url("chrome://newtab/"));
        assert!(!Allowlist::is_ephemeral_url("https://app.pulsex.com/"));
    }

    #[test]
    fn extension_json_roundtrip() {
        let a =
            Allowlist::from_url_and_hosts("https://app.pulsex.com/", &["ipfs.io".into()]).unwrap();
        let j = a.to_extension_json();
        assert!(j.contains("ipfs.io"));
        assert!(j.contains("pulsex.com"));
    }
}
