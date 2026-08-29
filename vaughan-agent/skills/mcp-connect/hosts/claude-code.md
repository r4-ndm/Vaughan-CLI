# Claude Code — Vaughan MCP

Claude Code (CLI) registers MCP servers from **user** or **project** config. Exact file paths vary by version — check `claude mcp --help` on your install.

## Typical config locations

| Scope | Common path |
|-------|-------------|
| **Project** | `.mcp.json` or entry in project Claude config (check repo docs) |
| **User** | `~/.claude.json` or MCP section in Claude Code settings |

When in doubt, run:

```bash
claude mcp list
```

## Example server entry

Same shape as Cursor / Claude Desktop:

```json
{
  "mcpServers": {
    "vaughan": {
      "command": "vaughan",
      "args": ["mcp", "--profile", "default"],
      "env": {
        "VAUGHAN_DAPP_BROWSER_CHROME": "/usr/bin/chromium",
        "VAUGHAN_DAPP_BROWSER_CDP_PORT": "9222"
      }
    }
  }
}
```

Dev (from repo root):

```json
"command": "cargo",
"args": ["run", "-q", "-p", "vaughan-cli", "--", "mcp", "--profile", "default"]
```

## First-time setup

1. Build/install Vaughan.
2. Add the server entry to your Claude Code MCP config.
3. **Start a new Claude Code session** in the project directory (or run MCP reload if your version supports it).
4. Confirm `claude mcp list` shows `vaughan`.
5. Unlock Vaughan (TUI or `vaughan serve`) for writes.

## Reconnect MCP (after config change)

Claude Code binds MCP at session start. After editing config:

1. **Exit the current Claude Code session** (`Ctrl+C` or `/exit`).
2. Start a new session: `claude` from the project root.

If your Claude Code build has an explicit reload command, use it — otherwise session restart is the reliable reconnect.

## Verify

In session, ask the agent to run:

```
get_control_plane_status
get_network
```

Terminal smoke (same process Claude spawns):

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_control_plane_status","arguments":{}}}' \
  | vaughan mcp --profile default
```

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `vaughan` not in `claude mcp list` | Config file wrong scope; restart session |
| Works in terminal, not in Claude | Path issue — use absolute `command` path |
| `ready_for_writes: false` | Run `vaughan serve` or unlock TUI |

Reference: [`docs/mcp.md`](../../../../docs/mcp.md) · [`docs/mcp-smoke.md`](../../../../docs/mcp-smoke.md).
