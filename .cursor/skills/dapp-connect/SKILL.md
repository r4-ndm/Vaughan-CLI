---
name: dapp-connect
description: >-
  Vaughan dApp URL connect playbooks (per-site inject/CSP/IPFS quirks). Use when
  debugging Web / vaughan-dapp-browser connect, “Injected” hangs, or adding a
  trusted dApp bookmark.
---

# Vaughan dApp connect

When working on dApp browser connect issues or new Web bookmarks, read:

1. [`vaughan-agent/skills/dapp-connect/SKILL.md`](../../../vaughan-agent/skills/dapp-connect/SKILL.md)
2. The matching file under [`vaughan-agent/skills/dapp-connect/sites/`](../../../vaughan-agent/skills/dapp-connect/sites/)

Do not invent per-site behavior — update the site playbook when you learn a new
quirk. Prefer browserless Pulse (Ag/Dex/MCP) unless the user needs the web UI.

Signing stays in the Vaughan TUI; never add auto-sign in the page.
