//! Page snapshot and visible-text collection over CDP.

use std::collections::HashSet;

use serde_json::Value;

use super::client::CdpPage;
use super::js;
use crate::error::WalletError;

/// Interactive element snapshot for agents (`browser_snapshot`).
pub async fn cdp_snapshot(cdp_http_url: &str) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    page.evaluate(&js::snapshot_refs()).await
}

fn merge_visible_lines(out: &mut Vec<String>, seen: &mut HashSet<String>, payload: &Value) {
    let Some(arr) = payload.get("lines").and_then(|v| v.as_array()) else {
        return;
    };
    for line in arr {
        if let Some(s) = line.as_str() {
            let t = s.trim();
            if t.len() >= 2 && seen.insert(t.to_string()) {
                out.push(t.to_string());
            }
        }
    }
}

/// Visible text lines from every frame (main document last, deduped).
pub(crate) async fn collect_visible_lines_all_frames(
    page: &mut CdpPage,
) -> Result<Vec<String>, WalletError> {
    let mut lines = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(frame_ids) = page.frame_ids().await {
        for fid in frame_ids {
            if let Ok(v) = page.evaluate_in_frame(&fid, js::VISIBLE_LINES).await {
                merge_visible_lines(&mut lines, &mut seen, &v);
            }
        }
    }
    if let Ok(v) = page.evaluate(js::VISIBLE_LINES).await {
        merge_visible_lines(&mut lines, &mut seen, &v);
    }
    Ok(lines)
}
