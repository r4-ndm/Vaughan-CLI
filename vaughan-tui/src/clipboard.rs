//! Clipboard helpers for copying addresses / keys without mouse-selecting box art.
//!
//! Prefer OSC 52 (terminal clipboard, works over SSH). Fall back to
//! `wl-copy` / `xclip` / `xsel` / `pbcopy` when available. No extra crates.

use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

/// Copy `text` to the clipboard. Never logs the payload.
pub fn copy_text(text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("nothing to copy".into());
    }
    let osc_ok = write_osc52(trimmed).is_ok() && io::stdout().is_terminal();
    let native_ok = try_native_clipboard(trimmed).is_ok();
    if osc_ok || native_ok {
        Ok(())
    } else {
        Err("clipboard unavailable (need a TTY for OSC 52, or wl-copy / xclip / pbcopy)".into())
    }
}

fn write_osc52(text: &str) -> io::Result<()> {
    let b64 = base64_encode(text.as_bytes());
    // BEL-terminated OSC 52 set clipboard (`c` = clipboard selection).
    let seq = format!("\x1b]52;c;{b64}\x07");
    let mut out = io::stdout().lock();
    out.write_all(seq.as_bytes())?;
    out.flush()
}

fn try_native_clipboard(text: &str) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
    ];
    for (bin, args) in candidates {
        if let Ok(mut child) = Command::new(bin)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                if stdin.write_all(text.as_bytes()).is_ok() {
                    drop(stdin);
                    if child.wait().map(|s| s.success()).unwrap_or(false) {
                        return Ok(());
                    }
                }
            }
        }
    }
    Err("no native clipboard tool".into())
}

/// Minimal Base64 (RFC 4648) encoder for OSC 52 — no extra dependency.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n =
            (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8) | u32::from(input[i + 2]);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(n & 0x3f) as usize] as char);
        i += 3;
    }
    match input.len() - i {
        1 => {
            let n = u32::from(input[i]) << 16;
            out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8);
            out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
