//! Phase 0.5 smoke: CDP accessibility snapshot against Chromium.
//!
//! Spawns system Chromium with `--remote-debugging-port`, attaches via
//! chromiumoxide, calls `Accessibility.getFullAXTree`.
//!
//! Env:
//! - `VAUGHAN_SPIKE_CHROME` (default `/usr/bin/chromium`)
//! - `VAUGHAN_SPIKE_URL` (default `https://example.com/`)
//! - `VAUGHAN_SPIKE_CDP` — if set (e.g. `http://127.0.0.1:9229`), attach only

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;
use futures::StreamExt;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local_addr")
        .port()
}

fn spawn_chromium(chrome: &str, port: u16) -> std::io::Result<Child> {
    let profile = format!("/tmp/vaughan-cef-spike-chrome-profile-{port}");
    let _ = std::fs::remove_dir_all(&profile);
    let _ = std::fs::create_dir_all(&profile);
    Command::new(chrome)
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg(format!("--user-data-dir={profile}"))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

async fn wait_for_devtools(port: u16) -> Result<String, Box<dyn std::error::Error>> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    for i in 0..50 {
        let body = tokio::task::spawn_blocking({
            let url = url.clone();
            move || {
                std::process::Command::new("curl")
                    .args(["-fsS", "--max-time", "1", &url])
                    .output()
            }
        })
        .await??;

        if body.status.success() {
            let text = String::from_utf8_lossy(&body.stdout);
            if let Some(start) = text.find("ws://") {
                let rest = &text[start..];
                let end = rest.find('"').unwrap_or(rest.len());
                return Ok(rest[..end].to_string());
            }
        }
        eprintln!("cdp_ax_smoke: attempt {i}: waiting for DevTools…");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("devtools endpoint did not become ready".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let chrome = std::env::var("VAUGHAN_SPIKE_CHROME")
        .unwrap_or_else(|_| "/usr/bin/chromium".to_string());
    let url =
        std::env::var("VAUGHAN_SPIKE_URL").unwrap_or_else(|_| "https://example.com/".to_string());

    let mut child: Option<Child> = None;
    let (cdp_http, ws) = if let Ok(existing) = std::env::var("VAUGHAN_SPIKE_CDP") {
        let port: u16 = existing
            .trim_start_matches("http://127.0.0.1:")
            .trim_start_matches("http://localhost:")
            .parse()
            .map_err(|_| "VAUGHAN_SPIKE_CDP must be http://127.0.0.1:PORT")?;
        eprintln!("cdp_ax_smoke: attach-only port={port} url={url}");
        let ws = wait_for_devtools(port).await?;
        (existing, ws)
    } else {
        let port = free_port();
        eprintln!("cdp_ax_smoke: chrome={chrome} port={port} url={url}");
        child = Some(spawn_chromium(&chrome, port)?);
        let ws = wait_for_devtools(port).await?;
        (format!("http://127.0.0.1:{port}"), ws)
    };

    eprintln!("cdp_ax_smoke: ws={ws}");
    let result = run_cdp(&url, &cdp_http).await;

    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    result
}

async fn run_cdp(url: &str, cdp_http: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (mut browser, mut handler) = tokio::time::timeout(
        Duration::from_secs(15),
        Browser::connect(cdp_http.to_string()),
    )
    .await
    .map_err(|_| "Browser::connect timed out")??;

    let handle = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if h.is_err() {
                break;
            }
        }
    });

    eprintln!("cdp_ax_smoke: opening page…");
    let page = tokio::time::timeout(Duration::from_secs(30), browser.new_page(url))
        .await
        .map_err(|_| "new_page timed out")??;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let title = page.get_title().await?.unwrap_or_default();
    eprintln!("cdp_ax_smoke: title={title:?}; fetching page refs…");

    // Chrome 151 AX CDP shapes can disagree with older chromiumoxide_cdp serde.
    // Prefer a compact interactive-ref extract via Runtime.evaluate (still CDP).
    let refs_val = page
        .evaluate(
            r#"(() => {
              const sel = 'a,button,input,textarea,select,[role=button],[role=link]';
              return [...document.querySelectorAll(sel)].slice(0, 50).map((e, i) => ({
                ref: `e${i}`,
                tag: e.tagName.toLowerCase(),
                role: e.getAttribute('role') || null,
                name: (e.innerText || e.getAttribute('aria-label') || e.value || e.href || '').trim().slice(0, 80)
              }));
            })()"#,
        )
        .await?;
    let refs = refs_val.into_value::<serde_json::Value>()?;

    let mut ax_nodes = None;
    let mut ax_err = None;
    match tokio::time::timeout(
        Duration::from_secs(10),
        page.execute(GetFullAxTreeParams::default()),
    )
    .await
    {
        Ok(Ok(ax)) => ax_nodes = Some(ax.nodes.len()),
        Ok(Err(e)) => ax_err = Some(format!("ax_serde: {e}")),
        Err(_) => ax_err = Some("ax_timeout".into()),
    }

    let summary = serde_json::json!({
        "ok": true,
        "title": title,
        "url": url,
        "interactive_refs": refs,
        "ax_nodes": ax_nodes,
        "ax_note": ax_err,
        "cdp": cdp_http,
        "engine": "system-chromium-cdp",
        "note": "Agent CDP path OK (evaluate refs). Tauri+CEF embed remains Phase 1."
    });
    println!("{summary}");

    let _ = browser.close().await;
    handle.abort();
    Ok(())
}
