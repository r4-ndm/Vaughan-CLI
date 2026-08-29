//! MCP browser control tools (VB / CDP B1).
//!
//! Opens or navigates the optional `vaughan-dapp-browser` shell. Never signs —
//! signing stays in the TUI/provider.

use serde_json::{json, Value};
use std::time::Duration;

use alloy::primitives::{Address, U256};
use vaughan_core::chains::evm::tokens_for_chain;
use vaughan_core::core::aggregator::{quote_aggregator, AggQuoteRequest, AggVenue, AGG_VENUES};
use vaughan_core::core::persistence::{default_ipfs_gateway_hosts, StateManager};
use vaughan_core::core::vb_browser::{
    allow_suffixes_for_profile, cdp_alive, cdp_current_page_url, cdp_list_pages, cdp_open_or_reuse,
    cdp_open_url, check_url_allowed, clear_target_pin, clear_vb_session, read_vb_session,
    resolve_cdp_port, spawn_cdp_port, spawn_dapp_browser, terminate_vb_process,
    vb_session_pid_matches, vb_session_provider_token_stale, wait_for_cdp,
    write_target_pin, VbSession,
};
use vaughan_core::core::vb_cdp::{self, ElementRef, SwapTokenSide};
use vaughan_core::error::WalletError;

use crate::McpContext;

fn empty_object_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// Optional `type_strategy` arg shared by the typing tools. `auto` keeps the
/// hardened pipeline; venue playbooks may call for `insert-text` when a
/// dApp's display mask and quote engine diverge under per-char key events.
fn parse_type_strategy(args: &Value) -> Result<vb_cdp::TypeStrategy, String> {
    match args.get("type_strategy").and_then(|v| v.as_str()) {
        None => Ok(vb_cdp::TypeStrategy::Auto),
        Some(raw) => vb_cdp::TypeStrategy::parse(raw).ok_or_else(|| {
            format!(
                "unknown type_strategy `{raw}` (auto | key-events | insert-text | native-setter)"
            )
        }),
    }
}

fn url_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "HTTPS URL (must pass VB allowlist)"
            }
        },
        "required": ["url"]
    })
}

/// MCP `tools/list` entries for browser B1 + B2 tools.
pub fn browser_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "browser_open",
            "description": "Open allowlisted URL in vaughan-dapp-browser (VB). CDP for agent navigation requires Settings toggle, `vaughan config agent-browser on`, or VAUGHAN_DAPP_BROWSER_CDP_PORT. Never signs.",
            "inputSchema": url_schema(),
        }),
        json!({
            "name": "browser_open_agg",
            "description": "Open an Ag-catalog aggregator swap UI in VB (human path — no developer API key). \
                Agent playbook: vaughan-agent/skills/vb-ag-quotes/SKILL.md (venue index: venues/INDEX.md). \
                Venues: squirrel, pulseswap, piteas, switch, 9mm, curv, internetmoney, libertyx. \
                EmpX/PortalX: use quote_swap (no web UI). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "venue": {
                        "type": "string",
                        "description": "Aggregator id — see vaughan-agent/skills/vb-ag-quotes/venues/INDEX.md"
                    },
                    "pls_hex": {
                        "type": "boolean",
                        "description": "Use PLS→HEX deep link when available (default true). Prefer setup_tokens for explicit picker flow."
                    },
                    "token_in": {
                        "type": "string",
                        "description": "Input token symbol (e.g. PLS, WPLS). With token_out, runs browser_setup_swap after open."
                    },
                    "token_out": {
                        "type": "string",
                        "description": "Output token symbol (e.g. HEX)"
                    },
                    "amount_in": {
                        "type": "string",
                        "description": "Sell amount in human units (default 1)"
                    },
                    "setup_tokens": {
                        "type": "boolean",
                        "description": "After open, select tokens + amount via UI pickers (default true when token_in/out set or pls_hex)"
                    },
                    "type_strategy": {
                        "type": "string",
                        "enum": ["auto", "key-events", "insert-text", "native-setter"],
                        "description": "Typing strategy override for the amount (default auto)"
                    }
                },
                "required": ["venue"]
            }),
        }),
        json!({
            "name": "browser_navigate",
            "description": "Navigate the active VB session to an allowlisted URL via CDP. Reuses an existing same-origin tab unless new_tab is true. Requires a running VB child with CDP.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "https:// URL (allowlisted hosts only)" },
                    "new_tab": { "type": "boolean", "description": "Force a fresh tab instead of reusing a same-origin one (default false)" }
                },
                "required": ["url"]
            }),
        }),
        json!({
            "name": "browser_status",
            "description": "VB session + CDP health: cdp_url, open pages, allowlist suffix count.",
            "inputSchema": empty_object_schema(),
        }),
        json!({
            "name": "browser_snapshot",
            "description": "Snapshot interactive page elements (refs e0..e49) via CDP, plus visible quote hints from body text (all frames). Requires browser_open + CDP. Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "token_out": {
                        "type": "string",
                        "description": "Output token symbol for quote parsing (default HEX)"
                    }
                }
            }),
        }),
        json!({
            "name": "browser_read_quote",
            "description": "Read visible swap quote text from the active VB page (all frames — includes non-interactive labels agents miss in browser_snapshot refs). \
                Pass expect_amount_in + expect_token_in to cross-check the page's sell-side $ valuation against an oracle price — flags suspected_amount_misparse \
                (venue input masks that silently shift the typed amount, e.g. ÷1000). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "token_out": {
                        "type": "string",
                        "description": "Output token symbol to parse (default HEX)"
                    },
                    "expect_amount_in": {
                        "type": "string",
                        "description": "Intended sell amount in human units (e.g. \"1000000\") — enables the misparse cross-check"
                    },
                    "expect_token_in": {
                        "type": "string",
                        "description": "Sell token symbol or address (e.g. PLS) — enables the misparse cross-check"
                    }
                }
            }),
        }),
        json!({
            "name": "browser_click",
            "description": "Click element by ref from browser_snapshot (e.g. e3). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Element ref from browser_snapshot (e0, e1, …)" }
                },
                "required": ["ref"]
            }),
        }),
        json!({
            "name": "browser_click_text",
            "description": "Click first visible element containing text (e.g. Vaughan, Injected, MetaMask in wallet modal). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Case-insensitive substring to match" }
                },
                "required": ["text"]
            }),
        }),
        json!({
            "name": "browser_type",
            "description": "Focus element ref and insert text. Set clear:true to replace React controlled inputs. Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string" },
                    "text": { "type": "string" },
                    "clear": { "type": "boolean", "description": "Clear field before typing (default false)" },
                    "type_strategy": {
                        "type": "string",
                        "enum": ["auto", "key-events", "insert-text", "native-setter"],
                        "description": "Typing strategy override (default auto). Venue playbooks may require insert-text when a dApp's display mask and quote engine diverge under per-char key events."
                    }
                },
                "required": ["ref", "text"]
            }),
        }),
        json!({
            "name": "browser_select_token",
            "description": "Open swap token picker on input or output leg and select symbol (PLS, HEX, WPLS, …). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Token ticker to select" },
                    "side": {
                        "type": "string",
                        "enum": ["input", "output"],
                        "description": "Swap leg — input (top) or output (bottom)"
                    }
                },
                "required": ["symbol", "side"]
            }),
        }),
        json!({
            "name": "browser_setup_swap",
            "description": "Explicit swap setup: select token_in, token_out via UI pickers, set amount_in, click quote CTA (Switch Now / Swap). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "token_in": { "type": "string", "description": "e.g. PLS or native" },
                    "token_out": { "type": "string", "description": "e.g. HEX" },
                    "amount_in": { "type": "string", "description": "Human amount (default 1)" },
                    "submit_quote": {
                        "type": "boolean",
                        "description": "Click Switch Now / Swap after amount (default true)"
                    },
                    "type_strategy": {
                        "type": "string",
                        "enum": ["auto", "key-events", "insert-text", "native-setter"],
                        "description": "Typing strategy override for the amount (default auto)"
                    }
                },
                "required": ["token_in", "token_out"]
            }),
        }),
        json!({
            "name": "browser_submit_swap",
            "description": "Click the quote/swap CTA (Switch.win: Switch Now; others: Swap). Use after tokens + amount are set. Never signs.",
            "inputSchema": empty_object_schema(),
        }),
        json!({
            "name": "browser_connect_wallet",
            "description": "Open Connect Wallet modal and select Vaughan/Injected (all frames + shadow DOM). Never signs — approve in TUI.",
            "inputSchema": empty_object_schema(),
        }),
        json!({
            "name": "browser_press",
            "description": "Press key on active page (Enter, Tab, Escape, Arrow*, Space, Backspace, Delete). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" }
                },
                "required": ["key"]
            }),
        }),
        json!({
            "name": "browser_wait",
            "description": "Wait until text, CSS selector, or URL substring matches (default timeout 10s). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "selector": { "type": "string" },
                    "url_contains": { "type": "string" },
                    "timeout_ms": { "type": "integer", "description": "Default 10000" }
                }
            }),
        }),
    ]
}

pub fn browser_tool_names() -> &'static [&'static str] {
    &[
        "browser_open",
        "browser_open_agg",
        "browser_navigate",
        "browser_status",
        "browser_snapshot",
        "browser_read_quote",
        "browser_click",
        "browser_click_text",
        "browser_type",
        "browser_select_token",
        "browser_setup_swap",
        "browser_submit_swap",
        "browser_connect_wallet",
        "browser_press",
        "browser_wait",
    ]
}

fn wallet_err(e: WalletError) -> String {
    e.user_message()
}

fn suffixes_for_nav(ctx: &McpContext, url: &str, session_suffixes: &[String]) -> Vec<String> {
    if !session_suffixes.is_empty() {
        return session_suffixes.to_vec();
    }
    allow_suffixes_for_profile(&ctx.profile, url).unwrap_or_default()
}

fn agent_control_enabled(profile: &str) -> bool {
    StateManager::agent_browser_control_for_profile(profile)
}

fn cdp_port_for_profile(profile: &str) -> u16 {
    resolve_cdp_port(agent_control_enabled(profile))
}

const AGENT_CONTROL_DISABLED: &str = "browser_unavailable: agent browser control disabled — enable in Settings (p), `vaughan config agent-browser on`, or set VAUGHAN_DAPP_BROWSER_CDP_PORT";

fn require_agent_browser_control(profile: &str) -> Result<(), String> {
    if cdp_port_for_profile(profile) == 0 {
        return Err(AGENT_CONTROL_DISABLED.to_string());
    }
    Ok(())
}

/// Verify a `vb.session` is live AND still owned by a `vaughan-dapp-browser`
/// process. A stale file plus a squatted CDP port (or the user's own Chromium
/// started with `--remote-debugging-port`) must never be driven as VB — the
/// PID binding is what distinguishes "our browser" from "something answered
/// on loopback". Fail-closed: on mismatch the stale files are removed.
async fn verify_vb_session(session: &VbSession) -> Result<(), String> {
    if !vb_session_pid_matches(session) {
        clear_vb_session();
        return Err(
            "browser_unavailable: stale vb.session (recorded PID is gone or is not vaughan-dapp-browser) — reopen with browser_open"
                .to_string(),
        );
    }
    if !cdp_alive(&session.cdp_url).await {
        return Err(
            "browser_unavailable: VB CDP endpoint not reachable — reopen with browser_open"
                .to_string(),
        );
    }
    if vb_session_provider_token_stale(session) {
        terminate_vb_process(session);
        return Err(
            "browser_unavailable: VB provider token stale (wallet was unlocked after VB launch) — reopen with browser_open"
                .to_string(),
        );
    }
    Ok(())
}

/// True when `url`'s host is a public IPFS gateway mirror. Auto-connecting
/// the wallet there is refused: gateways are third-party infrastructure that
/// can serve altered page JS, so wallet connection on a mirror is a human
/// decision made in the VB window, not an agent default.
fn is_ipfs_gateway_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str().map(|h| h.to_ascii_lowercase()) else {
        return false;
    };
    default_ipfs_gateway_hosts()
        .iter()
        .any(|g| host == *g || host.ends_with(&format!(".{g}")))
}

pub async fn browser_open(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing url".to_string())?;
    let suffixes = allow_suffixes_for_profile(&ctx.profile, url).map_err(wallet_err)?;
    check_url_allowed(url, &suffixes).map_err(wallet_err)?;

    let agent_control = agent_control_enabled(&ctx.profile);
    let cdp_port = spawn_cdp_port(agent_control);

    // Reuse live VB — each browser_open spawn stacks Chromium + extension shells.
    // The session must verify (PID-bound) before its endpoint is trusted.
    if agent_control {
        let session = read_vb_session()
            .ok()
            .flatten()
            .filter(|s| !s.cdp_url.trim().is_empty());
        if let Some(session) = session {
            if verify_vb_session(&session).await.is_ok() {
                let (target, reused_tab) = cdp_open_or_reuse(&session.cdp_url, url)
                    .await
                    .map_err(wallet_err)?;
                if let Some(id) = target {
                    let _ = write_target_pin(&session.cdp_url, &id);
                }
                return Ok(json!({
                    "status": "reused",
                    "url": url,
                    "cdp_url": session.cdp_url,
                    "cdp_alive": true,
                    "agent_browser_control": agent_control,
                    "allow_suffixes": suffixes.len(),
                    "reused_tab": reused_tab,
                    "hint": if reused_tab {
                        "VB running — reused the existing same-origin tab"
                    } else {
                        "VB already running — opened a new tab in the existing session"
                    },
                }));
            }
        }
    }

    clear_target_pin();
    spawn_dapp_browser(url, &suffixes, cdp_port).map_err(wallet_err)?;

    // Chromium cold start with the extension can take several seconds; poll
    // long enough that a successful spawn reports cdp_alive in one call.
    let cdp_url = format!("http://127.0.0.1:{cdp_port}");
    let alive = if cdp_port != 0 {
        wait_for_cdp(&cdp_url, Duration::from_secs(15)).await
    } else {
        false
    };

    // Pin the tab VB opened with so follow-up tools stick to it.
    if alive {
        if let Ok(pages) = cdp_list_pages(&cdp_url).await {
            if let Some(id) = pages
                .iter()
                .find(|p| p.get("type").and_then(|t| t.as_str()) == Some("page"))
                .and_then(|p| p.get("id"))
                .and_then(|i| i.as_str())
            {
                let _ = write_target_pin(&cdp_url, id);
            }
        }
    }

    Ok(json!({
        "status": "opened",
        "url": url,
        "cdp_port": cdp_port,
        "cdp_alive": alive,
        "agent_browser_control": agent_control,
        "allow_suffixes": suffixes.len(),
        "hint": if alive {
            "VB running with CDP — use browser_navigate / browser_status"
        } else if cdp_port == 0 {
            "VB opened without CDP — enable agent browser control in Settings (p) or `vaughan config agent-browser on`"
        } else {
            "VB spawned but CDP never came up — check ~/.local/share/vaughan-cli/vb.log"
        },
    }))
}

fn parse_agg_venue(raw: &str) -> Result<AggVenue, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "squirrel" | "squirrelswap" => Ok(AggVenue::SquirrelSwap),
        "pulseswap" | "pulse" => Ok(AggVenue::PulseSwap),
        "piteas" => Ok(AggVenue::Piteas),
        "switch" | "switchwin" | "switch.win" => Ok(AggVenue::SwitchWin),
        "empx" | "empseal" => Ok(AggVenue::Empseal),
        "9mm" | "9x" | "nine_mm" | "9mm9x" => Ok(AggVenue::NineMm9x),
        "curv" | "jolt" => Ok(AggVenue::Curv),
        "internetmoney" | "int.money" | "im" => Ok(AggVenue::InternetMoney),
        "libertyx" | "libertyswap" | "liberty" => Ok(AggVenue::LibertyX),
        "portalx" | "portal" => Ok(AggVenue::PortalX),
        other => {
            let known: Vec<_> = AGG_VENUES.iter().map(|v| v.label()).collect();
            Err(format!(
                "unknown aggregator venue '{other}' — use one of: {}",
                known.join(", ")
            ))
        }
    }
}

fn parse_swap_side(raw: &str) -> Result<SwapTokenSide, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "input" | "in" | "from" | "sell" => Ok(SwapTokenSide::Input),
        "output" | "out" | "to" | "buy" => Ok(SwapTokenSide::Output),
        other => Err(format!("invalid side '{other}' — use input or output")),
    }
}

/// Open the Ag-catalog web UI for an aggregator (VB human path).
pub async fn browser_open_agg(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let venue_raw = args
        .get("venue")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing venue".to_string())?;
    let venue = parse_agg_venue(venue_raw)?;
    let pls_hex = args
        .get("pls_hex")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let token_in = args
        .get("token_in")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let token_out = args
        .get("token_out")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let amount_in = args
        .get("amount_in")
        .and_then(|v| v.as_str())
        .unwrap_or("1")
        .to_string();
    let setup_tokens = args
        .get("setup_tokens")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| token_in.is_some() || token_out.is_some() || pls_hex);
    let type_strategy = parse_type_strategy(&args)?;
    let mut connect_wallet = args
        .get("connect_wallet")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| ctx.active_address.is_some());

    let (tin, tout) = if setup_tokens {
        (
            token_in.unwrap_or_else(|| "PLS".into()),
            token_out.unwrap_or_else(|| "HEX".into()),
        )
    } else {
        (token_in.unwrap_or_default(), token_out.unwrap_or_default())
    };

    let url = if pls_hex && !setup_tokens {
        venue.web_url_pls_hex().ok_or_else(|| {
            format!(
                "{} has no public swap web UI — use browserless quote_swap / Ag screen",
                venue.label()
            )
        })?
    } else if let Some(base) = venue.web_url() {
        base.to_string()
    } else {
        return Err(format!(
            "{} has no public swap web UI — use browserless quote_swap / Ag screen",
            venue.label()
        ));
    };

    let mut out = browser_open(json!({ "url": url }), ctx).await?;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("venue".to_string(), json!(venue.label()));
        obj.insert("access".to_string(), json!(format!("{:?}", venue.access())));
        obj.insert("url".to_string(), json!(url));
    }

    // IPFS gateway mirrors are third-party infra that can serve altered page
    // JS — never auto-connect the wallet there (human decision in the window).
    let mut ipfs_connect_blocked = false;
    if connect_wallet && is_ipfs_gateway_url(&url) {
        connect_wallet = false;
        ipfs_connect_blocked = true;
    }

    if (setup_tokens || connect_wallet)
        && out.get("cdp_alive").and_then(|v| v.as_bool()) == Some(true)
    {
        tokio::time::sleep(Duration::from_secs(4)).await;
        if let Ok(Some(session)) = read_vb_session().map_err(wallet_err) {
            match vb_cdp::cdp_dismiss_modals(&session.cdp_url).await {
                Ok(dismiss) => {
                    if let Some(obj) = out.as_object_mut() {
                        obj.insert("dismiss_modals".to_string(), dismiss);
                    }
                }
                Err(e) => {
                    if let Some(obj) = out.as_object_mut() {
                        obj.insert("dismiss_modals_error".to_string(), json!(e.user_message()));
                    }
                }
            }
            if connect_wallet {
                match vb_cdp::cdp_connect_vaughan_wallet(&session.cdp_url, Some(ctx.chain_id)).await
                {
                    Ok(connect) => {
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert("connect_wallet".to_string(), connect);
                        }
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Err(e) => {
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert("connect_wallet_error".to_string(), json!(e.user_message()));
                        }
                    }
                }
            }
            if setup_tokens {
                match vb_cdp::cdp_setup_swap_with_strategy(
                    &session.cdp_url,
                    &tin,
                    &tout,
                    &amount_in,
                    true,
                    type_strategy,
                )
                .await
                {
                    Ok(setup) => {
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert("setup_swap".to_string(), setup);
                            obj.insert("token_in".to_string(), json!(tin));
                            obj.insert("token_out".to_string(), json!(tout));
                            obj.insert("amount_in".to_string(), json!(amount_in));
                        }
                    }
                    Err(e) => {
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert("setup_swap_error".to_string(), json!(e.user_message()));
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if let Ok(quote) =
                    vb_cdp::cdp_read_quote(&session.cdp_url, Some(tout.as_str())).await
                {
                    if let Some(obj) = out.as_object_mut() {
                        obj.insert("quote".to_string(), quote);
                    }
                }
            }
        }
    }

    if ipfs_connect_blocked {
        if let Some(obj) = out.as_object_mut() {
            obj.insert(
                "connect_wallet".to_string(),
                json!({
                    "ok": false,
                    "skipped": "ipfs_gateway",
                    "note": "public IPFS gateway mirror — connect the wallet manually in the VB window if you trust this deployment",
                }),
            );
        }
    }

    Ok(out)
}

pub async fn browser_navigate(args: Value, ctx: &McpContext) -> Result<Value, String> {
    require_agent_browser_control(&ctx.profile)?;
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing url".to_string())?;

    let session = read_vb_session().map_err(wallet_err)?.ok_or_else(|| {
        "browser_unavailable: no vb.session — run browser_open first with CDP enabled".to_string()
    })?;
    verify_vb_session(&session).await?;

    let suffixes = suffixes_for_nav(ctx, url, &session.allow_suffixes);
    check_url_allowed(url, &suffixes).map_err(wallet_err)?;

    let new_tab = args
        .get("new_tab")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let (target, reused_tab) = if new_tab {
        (
            cdp_open_url(&session.cdp_url, url)
                .await
                .map_err(wallet_err)?,
            false,
        )
    } else {
        cdp_open_or_reuse(&session.cdp_url, url)
            .await
            .map_err(wallet_err)?
    };
    if let Some(id) = target {
        let _ = write_target_pin(&session.cdp_url, &id);
    }

    Ok(json!({
        "status": "navigated",
        "url": url,
        "reused_tab": reused_tab,
        "cdp_url": session.cdp_url,
    }))
}

pub async fn browser_status(ctx: &McpContext) -> Result<Value, String> {
    let agent_control = agent_control_enabled(&ctx.profile);
    let Some(session) = read_vb_session().map_err(wallet_err)? else {
        return Ok(json!({
            "available": false,
            "reason": "no_vb_session",
            "agent_browser_control": agent_control,
            "hint": if agent_control {
                "Call browser_open first"
            } else {
                "Agent browser control disabled — enable in Settings (p) or `vaughan config agent-browser on`"
            },
        }));
    };

    if !vb_session_pid_matches(&session) {
        clear_vb_session();
        return Ok(json!({
            "available": false,
            "reason": "stale_session",
            "agent_browser_control": agent_control,
            "hint": "vb.session PID is gone or is not vaughan-dapp-browser — browser_open again",
        }));
    }

    let alive = cdp_alive(&session.cdp_url).await;
    if !alive {
        return Ok(json!({
            "available": false,
            "reason": "cdp_unreachable",
            "agent_browser_control": agent_control,
            "cdp_url": session.cdp_url,
            "allow_suffixes_count": session.allow_suffixes.len(),
            "hint": "VB child not running or CDP port changed — browser_open again",
        }));
    }

    if !agent_control {
        return Ok(json!({
            "available": false,
            "reason": "agent_control_disabled",
            "agent_browser_control": false,
            "cdp_url": session.cdp_url,
            "allow_suffixes_count": session.allow_suffixes.len(),
            "hint": "Agent browser control disabled — enable in Settings (p) or `vaughan config agent-browser on`",
        }));
    }

    let pages = cdp_list_pages(&session.cdp_url).await.map_err(wallet_err)?;
    let rows: Vec<Value> = pages
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("page"))
        .map(|p| {
            json!({
                "url": p.get("url").and_then(|u| u.as_str()).unwrap_or(""),
                "title": p.get("title").and_then(|t| t.as_str()).unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "available": true,
        "agent_browser_control": agent_control,
        "cdp_url": session.cdp_url,
        "allow_suffixes_count": session.allow_suffixes.len(),
        "pages": rows,
    }))
}

async fn require_cdp_session(ctx: &McpContext) -> Result<String, String> {
    require_agent_browser_control(&ctx.profile)?;
    let session = read_vb_session().map_err(wallet_err)?.ok_or_else(|| {
        "browser_unavailable: no vb.session — run browser_open first with CDP enabled".to_string()
    })?;
    verify_vb_session(&session).await?;
    Ok(session.cdp_url)
}

/// Mutating tools (click / type / keypress / connect / submit) additionally
/// re-check the CURRENT page URL against the session allowlist before acting:
/// the in-tab nav gate is the primary control, this catches a gate bypass or
/// a tab the user manually steered somewhere the agent should not touch.
/// Fail-closed when the current URL cannot be determined.
async fn require_cdp_session_mut(ctx: &McpContext) -> Result<String, String> {
    let session = read_vb_session().map_err(wallet_err)?.ok_or_else(|| {
        "browser_unavailable: no vb.session — run browser_open first with CDP enabled".to_string()
    })?;
    require_agent_browser_control(&ctx.profile)?;
    verify_vb_session(&session).await?;
    let current = cdp_current_page_url(&session.cdp_url)
        .await
        .ok_or_else(|| {
            "browser_unavailable: cannot read current page URL (fail-closed for mutating tools)"
                .to_string()
        })?;
    check_url_allowed(&current, &session.allow_suffixes).map_err(|e| {
        format!(
            "refused: current page `{current}` is outside the VB allowlist ({})",
            e.user_message()
        )
    })?;
    Ok(session.cdp_url)
}

pub async fn browser_snapshot(args: Value, ctx: &McpContext) -> Result<Value, String> {
    // token_out optional — inferred from the page's `Get <SYM>` leg when omitted.
    let token_out = args.get("token_out").and_then(|v| v.as_str());
    let cdp_url = require_cdp_session(ctx).await?;
    let snap = vb_cdp::cdp_snapshot(&cdp_url).await.map_err(wallet_err)?;
    let quote = vb_cdp::cdp_read_quote(&cdp_url, token_out)
        .await
        .map_err(wallet_err)?;
    Ok(json!({
        "status": "snapshot",
        "cdp_url": cdp_url,
        "page": snap,
        "quote": quote,
    }))
}

pub async fn browser_read_quote(args: Value, ctx: &McpContext) -> Result<Value, String> {
    // token_out optional — inferred from the page's `Get <SYM>` leg when omitted.
    let token_out = args.get("token_out").and_then(|v| v.as_str());
    let cdp_url = require_cdp_session(ctx).await?;
    let mut quote = vb_cdp::cdp_read_quote(&cdp_url, token_out)
        .await
        .map_err(wallet_err)?;
    if let (Some(amount), Some(token)) = (
        args.get("expect_amount_in").and_then(|v| v.as_str()),
        args.get("expect_token_in").and_then(|v| v.as_str()),
    ) {
        let check = sell_value_check(&quote, amount, token, ctx).await;
        if let Some(obj) = quote.as_object_mut() {
            obj.insert("sell_check".to_string(), check);
        }
    }
    Ok(json!({
        "status": "quote",
        "cdp_url": cdp_url,
        "quote": quote,
    }))
}

/// Cross-check the page's sell-side `$` valuation against the intended amount
/// priced by a browserless oracle quote (EmpX). Venue-agnostic misparse
/// detection: input masks that silently shift the typed amount (observed
/// ÷1000) show up as a ratio far from 1. Never fails the read — problems
/// come back as `{ ok: false, note }`.
async fn sell_value_check(
    quote: &Value,
    expect_amount_in: &str,
    expect_token_in: &str,
    ctx: &McpContext,
) -> Value {
    let fail = |note: String| json!({ "ok": false, "note": note });
    let sell_usd = match quote.get("sell_usd").and_then(|v| v.as_f64()) {
        Some(v) => v,
        None => return fail("no sell-side $ estimate visible on page".into()),
    };
    let intended: f64 = match expect_amount_in.trim().parse::<f64>() {
        Ok(v) if v > 0.0 && v.is_finite() => v,
        _ => return fail(format!("unparseable expect_amount_in `{expect_amount_in}`")),
    };

    // Resolve the sell token: native, registry symbol, or registry address.
    let raw = expect_token_in.trim();
    let (token_addr, token_native, decimals, symbol) = {
        let registry = tokens_for_chain(ctx.chain_id);
        let by_symbol = |sym: &str| registry.iter().find(|t| t.symbol.eq_ignore_ascii_case(sym));
        if raw.eq_ignore_ascii_case("native") || raw.eq_ignore_ascii_case("pls") {
            (Address::ZERO, true, 18u8, "PLS".to_string())
        } else if let Some(t) = by_symbol(raw) {
            match t.address.parse::<Address>() {
                Ok(a) => (a, false, t.decimals, t.symbol.to_string()),
                Err(_) => return fail(format!("bad registry address for {raw}")),
            }
        } else if let Ok(a) = raw.parse::<Address>() {
            match registry
                .iter()
                .find(|t| t.address.eq_ignore_ascii_case(raw))
            {
                Some(t) => (a, false, t.decimals, t.symbol.to_string()),
                None => {
                    return fail(format!(
                        "`{raw}` not in the curated registry — pass a known symbol"
                    ))
                }
            }
        } else {
            return fail(format!("unknown expect_token_in `{raw}`"));
        }
    };

    // Stablecoins price themselves; everything else goes through EmpX.
    let unit_price_usd = if matches!(symbol.as_str(), "USDC" | "USDT" | "DAI") {
        1.0
    } else {
        let usdc = match tokens_for_chain(ctx.chain_id)
            .into_iter()
            .find(|t| t.symbol == "USDC")
        {
            Some(t) => t,
            None => return fail("no USDC in chain registry — cannot price".into()),
        };
        let usdc_addr: Address = match usdc.address.parse() {
            Ok(a) => a,
            Err(_) => return fail("bad registry USDC address".into()),
        };
        // 1k units: large enough to dodge rounding noise, small enough to
        // dodge price impact — this is a spot price, not a trade quote.
        let probe = U256::from(1000u128 * 10u128.pow(decimals as u32));
        // Read-only price probe: the recipient only shapes calldata, so a
        // locked wallet falls back to the dead address (zero address is
        // rejected as a recipient by some routers).
        let dead: Address = "0x000000000000000000000000000000000000dEaD"
            .parse()
            .unwrap_or(Address::ZERO);
        let recipient = ctx.active_address.unwrap_or(dead);
        let req = AggQuoteRequest {
            token_in: token_addr,
            token_out: usdc_addr,
            token_in_is_native: token_native,
            token_out_is_native: false,
            amount_in: probe,
            slippage_percent: 0.5,
            account: Some(recipient),
        };
        match quote_aggregator(AggVenue::Empseal, &req, ctx.chain_id, None, None).await {
            Ok(q) => {
                let out: f64 = match q.amount_out.to_string().parse() {
                    Ok(v) => v,
                    Err(_) => return fail("oracle amount_out unparseable".into()),
                };
                out / 1e6 / 1000.0
            }
            Err(e) => return fail(format!("oracle quote failed: {e}")),
        }
    };

    let expected_usd = intended * unit_price_usd;
    let mut check = vb_cdp::assess_sell_value(sell_usd, expected_usd);
    if let Some(obj) = check.as_object_mut() {
        obj.insert("ok".to_string(), json!(true));
        obj.insert("unit_price_usd".to_string(), json!(unit_price_usd));
        obj.insert("expect_token_in".to_string(), json!(symbol));
    }

    // Output-leg cross-check: when the buy token is a stablecoin, the quote's
    // best output should land near the expected value too. This catches
    // venues whose display layer and quote engine parse the amount
    // differently (9X: sell $ correct, quote computed on raw digits).
    let token_out_sym = quote
        .get("token_out")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let out_stable = matches!(
        token_out_sym.to_ascii_uppercase().as_str(),
        "USDC" | "USDT" | "DAI"
    );
    if out_stable {
        if let Some(best) = quote.get("best").and_then(|v| v.as_f64()) {
            let out_ratio = if expected_usd > 0.0 {
                best / expected_usd
            } else {
                0.0
            };
            let out_flag = !(0.5..=2.0).contains(&out_ratio);
            if let Some(obj) = check.as_object_mut() {
                obj.insert(
                    "out_check".to_string(),
                    json!({
                        "page_out": best,
                        "token_out": token_out_sym,
                        "expected_usd": expected_usd,
                        "ratio": out_ratio,
                        "suspected_amount_misparse": out_flag,
                    }),
                );
            }
        }
    }
    check
}

pub async fn browser_click(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let ref_raw = args
        .get("ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing ref".to_string())?;
    let element_ref = ElementRef::parse(ref_raw).map_err(wallet_err)?;
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_click(&cdp_url, element_ref)
        .await
        .map_err(wallet_err)?;
    Ok(json!({ "status": "clicked", "ref": ref_raw, "result": result }))
}

pub async fn browser_click_text(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing text".to_string())?;
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_click_by_text(&cdp_url, text)
        .await
        .map_err(wallet_err)?;
    Ok(json!({ "status": "clicked_text", "text": text, "result": result }))
}

pub async fn browser_type(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let ref_raw = args
        .get("ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing ref".to_string())?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing text".to_string())?;
    let clear = args.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
    let strategy = parse_type_strategy(&args)?;
    let element_ref = ElementRef::parse(ref_raw).map_err(wallet_err)?;
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_type_with_strategy(&cdp_url, element_ref, text, clear, strategy)
        .await
        .map_err(wallet_err)?;
    Ok(json!({
        "status": "typed",
        "ref": ref_raw,
        "result": result,
    }))
}

pub async fn browser_press(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing key".to_string())?;
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_press(&cdp_url, key).await.map_err(wallet_err)?;
    Ok(json!({ "status": "pressed", "key": key, "result": result }))
}

pub async fn browser_wait(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let text = args.get("text").and_then(|v| v.as_str());
    let selector = args.get("selector").and_then(|v| v.as_str());
    let url_contains = args.get("url_contains").and_then(|v| v.as_str());
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(10_000);
    let cdp_url = require_cdp_session(ctx).await?;
    let result = vb_cdp::cdp_wait(
        &cdp_url,
        text,
        selector,
        url_contains,
        Duration::from_millis(timeout_ms),
    )
    .await
    .map_err(wallet_err)?;
    Ok(json!({ "status": "wait_met", "result": result }))
}

pub async fn browser_select_token(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing symbol".to_string())?;
    let side_raw = args
        .get("side")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing side (input | output)".to_string())?;
    let side = parse_swap_side(side_raw)?;
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_select_swap_token(&cdp_url, symbol, side)
        .await
        .map_err(wallet_err)?;
    Ok(json!({
        "status": "token_selected",
        "symbol": vb_cdp::normalize_swap_symbol(symbol),
        "side": side_raw,
        "result": result,
    }))
}

pub async fn browser_setup_swap(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let token_in = args
        .get("token_in")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing token_in".to_string())?;
    let token_out = args
        .get("token_out")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing token_out".to_string())?;
    let amount_in = args
        .get("amount_in")
        .and_then(|v| v.as_str())
        .unwrap_or("1");
    let submit_quote = args
        .get("submit_quote")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let strategy = parse_type_strategy(&args)?;
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_setup_swap_with_strategy(
        &cdp_url,
        token_in,
        token_out,
        amount_in,
        submit_quote,
        strategy,
    )
    .await
    .map_err(wallet_err)?;
    Ok(json!({
        "status": "swap_setup",
        "token_in": vb_cdp::normalize_swap_symbol(token_in),
        "token_out": vb_cdp::normalize_swap_symbol(token_out),
        "amount_in": amount_in,
        "result": result,
    }))
}

pub async fn browser_connect_wallet(_args: Value, ctx: &McpContext) -> Result<Value, String> {
    let cdp_url = require_cdp_session_mut(ctx).await?;
    if let Some(current) = cdp_current_page_url(&cdp_url).await {
        if is_ipfs_gateway_url(&current) {
            return Err(format!(
                "refused: current page `{current}` is a public IPFS gateway mirror — \
                 gateways can serve altered page JS, so wallet connection there is a \
                 human decision in the VB window, not an agent action"
            ));
        }
    }
    let result = vb_cdp::cdp_connect_vaughan_wallet(&cdp_url, Some(ctx.chain_id))
        .await
        .map_err(wallet_err)?;
    Ok(json!({ "status": "connect_wallet", "result": result }))
}

pub async fn browser_submit_swap(_args: Value, ctx: &McpContext) -> Result<Value, String> {
    let cdp_url = require_cdp_session_mut(ctx).await?;
    let result = vb_cdp::cdp_click_swap_submit(&cdp_url)
        .await
        .map_err(wallet_err)?;
    Ok(json!({
        "status": "submitted",
        "result": result,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_defs_count() {
        assert_eq!(browser_tool_definitions().len(), browser_tool_names().len());
    }

    #[test]
    fn require_agent_control_blocks_when_off() {
        std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
        assert!(require_agent_browser_control("nonexistent-profile-toggle-off-test").is_err());
    }
}
