---
name: mcp-connect
description: >-
  Connect Vaughan MCP to Cursor, Claude, or Claude Code. Use when MCP tools missing,
  restart/reconnect MCP, wallet_locked, first-time setup, browser_* unavailable.
  Canonical playbook: vaughan-agent/skills/mcp-connect/SKILL.md
---

# MCP connect (Cursor pointer)

**Canonical playbook (any agent):**

[`vaughan-agent/skills/mcp-connect/SKILL.md`](../../../vaughan-agent/skills/mcp-connect/SKILL.md)

**Per-host reconnect steps:**

[`vaughan-agent/skills/mcp-connect/hosts/INDEX.md`](../../../vaughan-agent/skills/mcp-connect/hosts/INDEX.md)

**All skills index:**

[`vaughan-agent/skills/INDEX.md`](../../../vaughan-agent/skills/INDEX.md)

## Cursor reconnect (quick)

Settings → **Features → MCP** → toggle **`vaughan`** off → on (or restart Cursor).

Then verify: `get_control_plane_status` + `get_network`.

Read this skill **before** [`vb-ag-quotes`](../vb-ag-quotes/SKILL.md) for aggregator work.
