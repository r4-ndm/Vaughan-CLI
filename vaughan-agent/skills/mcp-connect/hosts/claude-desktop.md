# Claude Desktop — Vaughan MCP

Anthropic’s desktop app has **no in-app “toggle MCP”**. Config changes require editing JSON and **restarting the app**.

## Config file location

| OS | Path |
|----|------|
| **macOS** | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| **Windows** | `%APPDATA%\Claude\claude_desktop_config.json` |
| **Linux** | `~/.config/Claude/claude_desktop_config.json` |

Create the file if it does not exist.

## Example config

Adjust `command` to your installed `vaughan` path or dev binary:

```json
{
  "mcpServers": {
    "vaughan": {
      "command": "/home/USER/Desktop/Vaughan-CLI/target/debug/vaughan",
      "args": ["mcp", "--profile", "default"],
      "env": {
        "VAUGHAN_DAPP_BROWSER_CHROME": "/usr/bin/chromium",
        "VAUGHAN_DAPP_BROWSER_CDP_PORT": "9222"
      }
    }
  }
}
```

For a system-wide install:

```json
"command": "vaughan",
"args": ["mcp", "--profile", "default"]
```

Sentient profile (separate server name — do not mix with human seed):

```json
"vaughan-sentient": {
  "command": "vaughan",
  "args": ["mcp", "--profile", "sentient"]
}
```

## First-time setup

1. Install/build Vaughan (`cargo build -p vaughan-cli` or `cargo install --path vaughan-cli`).
2. Edit `claude_desktop_config.json` as above.
3. **Quit Claude Desktop completely** (not just close the window — exit from tray/menu).
4. Reopen Claude Desktop.
5. Start a new chat; look for the MCP/tools indicator that `vaughan` is connected.
6. Unlock Vaughan separately (TUI or `vaughan serve`) for write paths.

## Reconnect MCP (after any config change)

1. Save `claude_desktop_config.json`.
2. **Quit Claude Desktop fully** (check system tray).
3. Reopen Claude Desktop.

There is no lighter-weight reload in Claude Desktop today — app restart **is** the reconnect.

## Verify

In chat, ask Claude to call Vaughan tools (if your Claude version exposes MCP tools to the model):

- `get_network`
- `get_control_plane_status`

Or run manually in a terminal to test the same subprocess Claude spawns:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_network","arguments":{}}}' \
  | vaughan mcp --profile default
```

Expect one JSON line on stdout (logs on stderr).

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| MCP section empty in Claude | JSON syntax error in config — validate JSON |
| Server fails silently | Use full path to `vaughan` binary in `command` |
| `wallet_locked` in tool results | Start `vaughan` TUI or `vaughan serve` |
| No browser tools | Add `VAUGHAN_DAPP_BROWSER_CDP_PORT` env; install `vaughan-dapp-browser` |

Reference: [Anthropic MCP docs](https://modelcontextprotocol.io) · Vaughan: [`docs/mcp.md`](../../../../docs/mcp.md).
