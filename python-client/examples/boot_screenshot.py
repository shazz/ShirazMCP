#!/usr/bin/env python3
"""Minimal example: boot a program and grab a screenshot.

    python3 examples/boot_screenshot.py /path/to/harddrive [out.png]

The harddrive is a folder with an AUTO/ subfolder holding the .PRG to auto-run.
Requires: a built shiraz-mcp binary, a live X display, and a TOS ROM (point at
it with SHIRAZMCP_TOS). See ../README.md.
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from mcp_client import McpClient  # noqa: E402


def find_binary() -> str:
    if os.environ.get("SHIRAZMCP_BIN"):
        return os.environ["SHIRAZMCP_BIN"]
    root = os.path.join(os.path.dirname(__file__), "..", "..")
    for prof in ("release", "debug"):
        p = os.path.join(root, "target", prof, "shiraz-mcp")
        if os.path.exists(p):
            return p
    sys.exit("no shiraz-mcp binary — run `cargo build --release` or set SHIRAZMCP_BIN")


def main() -> None:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    hd = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else "screenshot.png"

    env = dict(os.environ)
    env.setdefault("DISPLAY", ":0")
    c = McpClient(find_binary(), env=env)
    try:
        c.initialize()
        print("booting…", c.call("start", {"harddrive": hd, "machine": "ste", "memsize_mb": 1}))
        c.call("run_frames", {"frames": 400}, timeout=40)   # let it settle
        c.screenshot_to(out)
        print(f"screenshot -> {out}")
        print("machine:", c.call("machine_state"))
    finally:
        c.call("stop")
        c.close()


if __name__ == "__main__":
    main()
