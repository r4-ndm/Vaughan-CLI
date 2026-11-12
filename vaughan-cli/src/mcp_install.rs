//! MCP client configuration installer (`vaughan mcp install`).
//!
//! Registers Vaughan MCP server with external agent clients (Cursor, Claude,
//! Codex, Claude Desktop) without requiring hand-edited JSON or TOML.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

#[derive(Debug, Subcommand)]
pub enum McpCmd {
    /// Install Vaughan MCP configuration for external agent clients.
    Install {
        /// Target client: cursor, claude, codex, claude-desktop, or all.
        #[arg(long, value_enum)]
        client: ClientTarget,

        /// Profile name (default: "default").
        #[arg(long)]
        profile: Option<String>,

        /// Print what would be written without modifying any files.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ClientTarget {
    Cursor,
    Claude,
    Codex,
    ClaudeDesktop,
    All,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Configured(PathBuf),
    AlreadyConfigured(PathBuf),
    Skipped(String),
    DryRun(PathBuf, String),
}

/// Merge a server entry into an existing JSON config string.
///
/// Returns `Ok(None)` when the file already contains an identical entry (no write).
/// Returns `Err` if `existing` is not valid JSON.
pub fn merge_json_entry(existing: &str, key: &str, entry: Value) -> Result<Option<String>, String> {
    if existing.trim().is_empty() {
        let mut servers = serde_json::Map::new();
        servers.insert(key.to_string(), entry);
        let mut root = serde_json::Map::new();
        root.insert("mcpServers".to_string(), Value::Object(servers));
        let pretty = serde_json::to_string_pretty(&Value::Object(root))
            .map_err(|e| format!("json serialize: {e}"))?;
        return Ok(Some(format!("{pretty}\n")));
    }

    let mut root: Value = serde_json::from_str(existing).map_err(|e| format!("parse json: {e}"))?;

    let obj = root
        .as_object_mut()
        .ok_or_else(|| "root is not a JSON object".to_string())?;

    let servers_val = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    let servers = servers_val
        .as_object_mut()
        .ok_or_else(|| "mcpServers is not an object".to_string())?;

    if servers.get(key) == Some(&entry) {
        return Ok(None);
    }

    servers.insert(key.to_string(), entry);
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| format!("json serialize: {e}"))?;
    Ok(Some(format!("{pretty}\n")))
}

/// Merge a server entry into an existing TOML config string for Codex.
///
/// Returns `Ok(None)` when `[mcp_servers.<key>]` already contains an identical entry.
/// Returns `Err` if `existing` is not valid TOML.
pub fn merge_toml_entry(existing: &str, key: &str, entry: &str) -> Result<Option<String>, String> {
    let entry_val: toml::Value = entry
        .parse()
        .map_err(|e| format!("invalid entry toml: {e}"))?;

    let mut root: toml::Value = if existing.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        existing.parse().map_err(|e| format!("parse toml: {e}"))?
    };

    let table = root
        .as_table_mut()
        .ok_or_else(|| "root is not a toml table".to_string())?;

    let servers_val = table
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

    let servers = servers_val
        .as_table_mut()
        .ok_or_else(|| "mcp_servers is not a table".to_string())?;

    if servers.get(key) == Some(&entry_val) {
        return Ok(None);
    }

    servers.insert(key.to_string(), entry_val);
    let rendered = toml::to_string_pretty(&root).map_err(|e| format!("toml serialize: {e}"))?;
    Ok(Some(rendered))
}

/// Atomic write via unique scratch file and rename (mode 0600 on Unix).
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = format!(
        "tmp.{:x}.{:x}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let tmp = path.with_extension(unique);
    {
        let mut file =
            fs::File::create(&tmp).map_err(|e| format!("create tmp {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        file.write_all(content.as_bytes())
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))?;
    Ok(())
}

/// Install JSON entry to a target path with validation, backup, and atomic write.
pub fn install_json_target(
    path: &Path,
    key: &str,
    entry: Value,
    dry_run: bool,
) -> Result<InstallOutcome, String> {
    let existing = if path.exists() {
        fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?
    } else {
        String::new()
    };

    let merged = match merge_json_entry(&existing, key, entry) {
        Ok(m) => m,
        Err(_) => {
            return Err(format!(
                "mcp_install_invalid_config: {} is not valid JSON — refusing to rewrite it",
                path.display()
            ));
        }
    };

    match merged {
        None => Ok(InstallOutcome::AlreadyConfigured(path.to_path_buf())),
        Some(new_content) => {
            if dry_run {
                Ok(InstallOutcome::DryRun(path.to_path_buf(), new_content))
            } else {
                if path.exists() {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let bak_path = format!("{}.vaughan-bak-{}", path.display(), now);
                    fs::copy(path, &bak_path)
                        .map_err(|e| format!("backup {}: {e}", path.display()))?;
                }
                write_atomic(path, &new_content)?;
                Ok(InstallOutcome::Configured(path.to_path_buf()))
            }
        }
    }
}

/// Install TOML entry to a target path with validation, backup, and atomic write.
pub fn install_toml_target(
    path: &Path,
    key: &str,
    entry_toml: &str,
    dry_run: bool,
) -> Result<InstallOutcome, String> {
    let existing = if path.exists() {
        fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?
    } else {
        String::new()
    };

    let merged = match merge_toml_entry(&existing, key, entry_toml) {
        Ok(m) => m,
        Err(_) => {
            return Err(format!(
                "mcp_install_invalid_config: {} is not valid TOML — refusing to rewrite it",
                path.display()
            ));
        }
    };

    match merged {
        None => Ok(InstallOutcome::AlreadyConfigured(path.to_path_buf())),
        Some(new_content) => {
            if dry_run {
                Ok(InstallOutcome::DryRun(path.to_path_buf(), new_content))
            } else {
                if path.exists() {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let bak_path = format!("{}.vaughan-bak-{}", path.display(), now);
                    fs::copy(path, &bak_path)
                        .map_err(|e| format!("backup {}: {e}", path.display()))?;
                }
                write_atomic(path, &new_content)?;
                Ok(InstallOutcome::Configured(path.to_path_buf()))
            }
        }
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "HOME environment variable not set".to_string())
}

fn claude_binary_exists() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn claude_mcp_get_vaughan() -> bool {
    std::process::Command::new("claude")
        .args(["mcp", "get", "vaughan"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_claude_mcp_add_json(entry_json: &str) -> Result<(), String> {
    let output = std::process::Command::new("claude")
        .args(["mcp", "add-json", "-s", "user", "vaughan", entry_json])
        .output()
        .map_err(|e| format!("failed to execute claude CLI: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("claude mcp add-json failed: {stderr}"))
    }
}

pub fn run_install(client: ClientTarget, profile: &str, dry_run: bool) -> anyhow::Result<()> {
    let home = home_dir().map_err(|e| anyhow::anyhow!("{e}"))?;

    let cursor_path = home.join(".cursor").join("mcp.json");
    let claude_path = home.join(".claude.json");
    let codex_path = home.join(".codex").join("config.toml");
    let claude_desktop_path = home
        .join(".config")
        .join("Claude")
        .join("claude_desktop_config.json");

    let entry_json = json!({
        "command": "vaughan",
        "args": ["mcp", "--profile", profile]
    });
    let entry_json_str = entry_json.to_string();

    let entry_toml =
        format!("command = \"vaughan\"\nargs = [\"mcp\", \"--profile\", \"{profile}\"]\n");

    let mut outcomes: Vec<InstallOutcome> = Vec::new();

    let install_cursor = |dry: bool| -> Result<InstallOutcome, String> {
        install_json_target(&cursor_path, "vaughan", entry_json.clone(), dry)
    };

    let install_codex = |dry: bool| -> Result<InstallOutcome, String> {
        install_toml_target(&codex_path, "vaughan", &entry_toml, dry)
    };

    let install_claude_desktop = |dry: bool| -> Result<InstallOutcome, String> {
        install_json_target(&claude_desktop_path, "vaughan", entry_json.clone(), dry)
    };

    let install_claude = |dry: bool| -> Result<InstallOutcome, String> {
        if claude_path.exists() {
            let existing = fs::read_to_string(&claude_path)
                .map_err(|e| format!("read {}: {e}", claude_path.display()))?;
            if !existing.trim().is_empty() && serde_json::from_str::<Value>(&existing).is_err() {
                return Err(format!(
                    "mcp_install_invalid_config: {} is not valid JSON — refusing to rewrite it",
                    claude_path.display()
                ));
            }
        }

        if claude_binary_exists() {
            if dry {
                return Ok(InstallOutcome::DryRun(
                    claude_path.clone(),
                    format!("claude mcp add-json -s user vaughan '{entry_json_str}'"),
                ));
            }
            if claude_mcp_get_vaughan() {
                return Ok(InstallOutcome::AlreadyConfigured(claude_path.clone()));
            }
            if claude_path.exists() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let bak_path = format!("{}.vaughan-bak-{}", claude_path.display(), now);
                fs::copy(&claude_path, &bak_path)
                    .map_err(|e| format!("backup {}: {e}", claude_path.display()))?;
            }
            match run_claude_mcp_add_json(&entry_json_str) {
                Ok(()) => return Ok(InstallOutcome::Configured(claude_path.clone())),
                Err(_) => {
                    // Fall back to writing ~/.claude.json
                }
            }
        }
        install_json_target(&claude_path, "vaughan", entry_json.clone(), dry)
    };

    match client {
        ClientTarget::Cursor => {
            outcomes.push(install_cursor(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
        }
        ClientTarget::Claude => {
            outcomes.push(install_claude(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
        }
        ClientTarget::Codex => {
            outcomes.push(install_codex(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
        }
        ClientTarget::ClaudeDesktop => {
            outcomes.push(install_claude_desktop(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
        }
        ClientTarget::All => {
            // Cursor: directory is ~/.cursor
            if cursor_path.parent().map(|p| p.exists()).unwrap_or(false) {
                outcomes.push(install_cursor(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
            } else {
                outcomes.push(InstallOutcome::Skipped(
                    "cursor: ~/.cursor directory does not exist".into(),
                ));
            }

            // Claude: check binary or directory
            if claude_binary_exists() || claude_path.exists() || home.join(".claude").exists() {
                outcomes.push(install_claude(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
            } else {
                outcomes.push(InstallOutcome::Skipped(
                    "claude: claude CLI or ~/.claude not found".into(),
                ));
            }

            // Codex: directory is ~/.codex
            if codex_path.parent().map(|p| p.exists()).unwrap_or(false) {
                outcomes.push(install_codex(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
            } else {
                outcomes.push(InstallOutcome::Skipped(
                    "codex: ~/.codex directory does not exist".into(),
                ));
            }

            // Claude Desktop: directory is ~/.config/Claude
            if claude_desktop_path
                .parent()
                .map(|p| p.exists())
                .unwrap_or(false)
            {
                outcomes.push(install_claude_desktop(dry_run).map_err(|e| anyhow::anyhow!("{e}"))?);
            } else {
                outcomes.push(InstallOutcome::Skipped(
                    "claude-desktop: ~/.config/Claude directory does not exist".into(),
                ));
            }
        }
    }

    for outcome in outcomes {
        match outcome {
            InstallOutcome::Configured(path) => println!("configured {}", path.display()),
            InstallOutcome::AlreadyConfigured(path) => {
                println!("already configured {}", path.display())
            }
            InstallOutcome::Skipped(reason) => println!("skipped {reason}"),
            InstallOutcome::DryRun(path, snippet) => {
                println!("dry-run {}:\n{snippet}", path.display())
            }
        }
    }

    println!("Reminder: for autonomous execution run `vaughan serve --password-env …` (or keep the TUI unlocked); the MCP process only proposes.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_empty_json_file() {
        let entry = json!({
            "command": "vaughan",
            "args": ["mcp", "--profile", "default"]
        });
        let result = merge_json_entry("", "vaughan", entry.clone()).unwrap();
        assert!(result.is_some());
        let content = result.unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["mcpServers"]["vaughan"], entry);
    }

    #[test]
    fn test_empty_toml_file() {
        let entry = "command = \"vaughan\"\nargs = [\"mcp\", \"--profile\", \"default\"]\n";
        let result = merge_toml_entry("", "vaughan", entry).unwrap();
        assert!(result.is_some());
        let content = result.unwrap();
        let parsed: toml::Value = content.parse().unwrap();
        assert_eq!(
            parsed["mcp_servers"]["vaughan"]["command"]
                .as_str()
                .unwrap(),
            "vaughan"
        );
        assert_eq!(
            parsed["mcp_servers"]["vaughan"]["args"][0]
                .as_str()
                .unwrap(),
            "mcp"
        );
    }

    #[test]
    fn test_json_file_with_other_servers_preserved() {
        let existing = r#"{
  "mcpServers": {
    "other_server": {
      "command": "other",
      "args": ["run"]
    }
  }
}"#;
        let entry = json!({
            "command": "vaughan",
            "args": ["mcp", "--profile", "default"]
        });
        let result = merge_json_entry(existing, "vaughan", entry.clone()).unwrap();
        assert!(result.is_some());
        let content = result.unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["mcpServers"]["vaughan"], entry);
        assert_eq!(parsed["mcpServers"]["other_server"]["command"], "other");
    }

    #[test]
    fn test_duplicate_entry_no_write() {
        let entry = json!({
            "command": "vaughan",
            "args": ["mcp", "--profile", "default"]
        });
        let existing = serde_json::to_string(&json!({
            "mcpServers": {
                "vaughan": entry
            }
        }))
        .unwrap();

        let result = merge_json_entry(&existing, "vaughan", entry).unwrap();
        assert_eq!(result, None);

        let entry_toml = "command = \"vaughan\"\nargs = [\"mcp\", \"--profile\", \"default\"]\n";
        let existing_toml = "[mcp_servers.vaughan]\ncommand = \"vaughan\"\nargs = [\"mcp\", \"--profile\", \"default\"]\n";
        let result_toml = merge_toml_entry(existing_toml, "vaughan", entry_toml).unwrap();
        assert_eq!(result_toml, None);
    }

    #[test]
    fn test_malformed_json_refusal_naming_path() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp.json");
        fs::write(&config_path, "not valid json {[[[").unwrap();

        let entry = json!({
            "command": "vaughan",
            "args": ["mcp", "--profile", "default"]
        });
        let err = install_json_target(&config_path, "vaughan", entry, false).unwrap_err();
        assert_eq!(
            err,
            format!(
                "mcp_install_invalid_config: {} is not valid JSON — refusing to rewrite it",
                config_path.display()
            )
        );
        // Original malformed file was not modified
        assert_eq!(
            fs::read_to_string(&config_path).unwrap(),
            "not valid json {[[["
        );
    }

    #[test]
    fn test_dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp.json");
        let entry = json!({
            "command": "vaughan",
            "args": ["mcp", "--profile", "default"]
        });

        let outcome = install_json_target(&config_path, "vaughan", entry, true).unwrap();
        assert!(matches!(outcome, InstallOutcome::DryRun(..)));
        assert!(
            !config_path.exists(),
            "dry run must not create or write files"
        );
    }

    #[test]
    fn test_toml_merge_for_codex_keeps_other_sections() {
        let existing = r#"[general]
model = "gpt-4o"
timeout = 30

[env]
DEBUG = "1"
"#;
        let entry_toml = "command = \"vaughan\"\nargs = [\"mcp\", \"--profile\", \"default\"]\n";
        let result = merge_toml_entry(existing, "vaughan", entry_toml).unwrap();
        assert!(result.is_some());
        let content = result.unwrap();

        let parsed: toml::Value = content.parse().unwrap();
        assert_eq!(parsed["general"]["model"].as_str().unwrap(), "gpt-4o");
        assert_eq!(parsed["general"]["timeout"].as_integer().unwrap(), 30);
        assert_eq!(parsed["env"]["DEBUG"].as_str().unwrap(), "1");
        assert_eq!(
            parsed["mcp_servers"]["vaughan"]["command"]
                .as_str()
                .unwrap(),
            "vaughan"
        );
        assert_eq!(
            parsed["mcp_servers"]["vaughan"]["args"][0]
                .as_str()
                .unwrap(),
            "mcp"
        );
    }
}
