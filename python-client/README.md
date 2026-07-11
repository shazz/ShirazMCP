# ShirazMCP Python client

A tiny, dependency-free Python client for driving the `shiraz-mcp` server from a
script — no MCP-aware agent required. It spawns the binary and speaks the same
line-delimited JSON-RPC 2.0 over stdio the protocol uses.

This is how ShirazMCP itself was developed and tested (see `DEVELOPMENT.md`): while
the binary is still changing, driving it directly beats registering it as an MCP
server that hands agents stale tools after every rebuild.

## Use it

```python
import os
from mcp_client import McpClient

c = McpClient("../target/release/shiraz-mcp", env={**os.environ, "DISPLAY": ":0"})
c.initialize()
c.call("start", {"harddrive": "/path/to/hd", "machine": "ste", "memsize_mb": 1})
c.call("run_frames", {"frames": 400})
c.screenshot_to("shot.png")
print(c.call("machine_state"))
c.call("stop")           # always safe; call it in a finally
```

`call(name, args)` returns the tool's structured JSON (or `{"_image_b64": …}` for
`screenshot`, or `{"_text": …}` for plain text). Every one of the server's tools is
reachable this way.

## Prerequisites

- A built binary: `cargo build --release` (or set `SHIRAZMCP_BIN`).
- A live **X display** (`DISPLAY`) — Hatari needs one.
- A **TOS ROM**: point at it with `SHIRAZMCP_TOS=/path/to/tos.img`.
- Optionally `SHIRAZMCP_HATARI` if `hatari` isn't at `/usr/bin/hatari`.

## Examples

| Script | What it does |
|---|---|
| `examples/boot_screenshot.py` | boot a harddrive's `AUTO/*.PRG`, screenshot it, dump machine state |
| `examples/identify_tune.py` | **Shazam-for-SNDH** — find the demo's embedded SNDH music in RAM and identify the tune by its header tags *and* by fingerprint-matching `../../db/` |

```sh
python3 examples/boot_screenshot.py /path/to/harddrive shot.png
python3 examples/identify_tune.py  /path/to/harddrive
```

Both accept `SHIRAZMCP_BIN` / `SHIRAZMCP_TOS` from the environment.
