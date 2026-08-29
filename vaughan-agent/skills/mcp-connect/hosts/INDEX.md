# MCP host index

Step-by-step **reconnect** and **first-time config** for each agent host.

| Host | File | Config location |
|------|------|-----------------|
| **Cursor** | [`cursor.md`](cursor.md) | `.cursor/mcp.json` (project) or Cursor Settings → MCP |
| **Claude Desktop** | [`claude-desktop.md`](claude-desktop.md) | `claude_desktop_config.json` |
| **Claude Code** | [`claude-code.md`](claude-code.md) | Project or user MCP config (CLI) |

Canonical workflow: [`../SKILL.md`](../SKILL.md).

After any edit to MCP config, env vars, or `vaughan config agent-browser` → **reconnect**
using the host file above, then verify with `get_control_plane_status`.
