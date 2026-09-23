#!/usr/bin/env python3
"""Trigger Vaughan personal_sign for Trezor smoke (ws://127.0.0.1:8745)."""

import asyncio
import json
from pathlib import Path

import websockets

# Paste your F3 Trezor address between the quotes:
ADDR = "YOUR_TREZOR_ADDRESS"

TOKEN = Path.home().joinpath(".local/share/vaughan-cli/provider.session").read_text().strip()
# Plain UTF-8 (not 0x-hex) so it cannot be mistaken for an address.
MSG = "vaughan-trezor-personal-sign-smoke"
URL = f"ws://127.0.0.1:8745/?access_token={TOKEN}"
# Bridge allowlist requires a trusted Origin (bare local clients are rejected).
ORIGIN = "https://freedom.browser"


async def main() -> None:
    # PIN + on-device confirm can take minutes; default websockets ping (~20s) dies first.
    async with websockets.connect(
        URL,
        additional_headers=[
            ("Origin", ORIGIN),
            ("Host", "127.0.0.1:8745"),
        ],
        ping_interval=60,
        ping_timeout=120,
        close_timeout=30,
        open_timeout=30,
    ) as ws:
        await ws.send(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "personal_sign",
                    "params": [MSG, ADDR],
                }
            )
        )
        print("Waiting for Vaughan approve → PIN → confirm on Trezor…", flush=True)
        # Hard cap for the whole human flow (10 minutes).
        print(await asyncio.wait_for(ws.recv(), timeout=600))


if __name__ == "__main__":
    if ADDR == "YOUR_TREZOR_ADDRESS":
        raise SystemExit(
            "Edit ADDR in this file to your F3 Trezor address, then re-run:\n"
            "  python3 scripts/trezor_personal_sign.py"
        )
    asyncio.run(main())
