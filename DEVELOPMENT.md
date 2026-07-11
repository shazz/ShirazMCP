# Development — driving ShirazMCP while you hack on it

ShirazMCP is an MCP server: a binary that speaks the Model Context Protocol over
**stdio**. This doc is about running and iterating on it *during development* —
how to drive it by hand, and how (and when) to wire it into an agent.

For the tool surface and Atari-side workflows, see `README.md`, the `shirazmcp`
skill, and `docs/live-patching.md`. For architecture, see `ARCHITECTURE.md`.

## Prerequisites

- The `hatari` binary — default `/usr/bin/hatari` (overridable in `HatariConfig`).
- A live **X display** — `DISPLAY` must point at a running X server. Hatari needs
  one to render; there is no headless (`Xvfb`) path wired up yet.
- A **TOS ROM** image — default `HatariConfig::tos_path` (see below).
- Rust 1.92 (2021 edition).

## Build

```sh
cargo build --release        # -> target/release/shiraz-mcp
```

Config is **compiled-in** — there is no config file yet. The defaults live in
`HatariConfig::default()` (`src/session/spawn/mod.rs`):

| Field | Default |
|---|---|
| `hatari_path` | `/usr/bin/hatari` |
| `tos_path` | `$HOME/projects/MJJ/bin/hatari/TOS/tos162fr.img` |
| `memsize_mb` | `1` |

Change those there until a runtime-config surface exists. `start`'s per-call
params (`harddrive`, `machine`, `memsize_mb`) override the boot for that session.

## Two ways to drive it in dev

### 1. Harness mode — spawn the binary, talk JSON-RPC (use this while iterating)

**Prefer this while the binary is still changing.** A registered MCP server is
resolved by the client at startup, so every `cargo build` leaves a live agent
holding *stale* tools until it reconnects. Spawning the freshly-built binary
per run sidesteps that entirely — no client, no config, no reconnect.

The protocol is **line-delimited JSON-RPC 2.0 over stdio** (one JSON object per
line, `\n`-terminated — *no* `Content-Length` framing; this matches rmcp's
`transport::io::stdio()` and `tests/mcp_handshake.rs`). tracing/log output goes
to **stderr**, so stdout stays clean for the protocol.

Handshake: `initialize` → `notifications/initialized` → then `tools/call`.

A minimal client (self-contained; adapt as a throwaway harness):

```python
import json, subprocess, base64

class Mcp:
    def __init__(self, binary, env=None):
        self.p = subprocess.Popen([binary], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, env=env, bufsize=0)
        self.i = 0

    def _send(self, o): self.p.stdin.write((json.dumps(o) + "\n").encode()); self.p.stdin.flush()
    def _recv(self):    return json.loads(self.p.stdout.readline())

    def initialize(self):
        self.i += 1
        self._send({"jsonrpc": "2.0", "id": self.i, "method": "initialize",
                    "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                               "clientInfo": {"name": "dev", "version": "0"}}})
        r = self._recv()
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return r

    def call(self, name, args=None):
        self.i += 1
        self._send({"jsonrpc": "2.0", "id": self.i, "method": "tools/call",
                    "params": {"name": name, "arguments": args or {}}})
        r = self._recv()
        if "error" in r or r.get("result", {}).get("isError"):
            raise RuntimeError(r)
        res = r["result"]
        # structured JSON if present, else the text/image content blocks
        if res.get("structuredContent") is not None:
            return res["structuredContent"]
        return res.get("content", [])

import os
env = dict(os.environ); env["DISPLAY"] = ":0"
c = Mcp("target/release/shiraz-mcp", env=env)
c.initialize()
print(c.call("start", {"harddrive": "/path/to/hd", "machine": "ste", "memsize_mb": 1}))
print(c.call("status"))
# screenshot returns an image content block: {"type":"image","data":<base64>,...}
img = next(b for b in c.call("screenshot") if b.get("type") == "image")
open("shot.png", "wb").write(base64.b64decode(img["data"]))
c.call("stop")
```

Notes:
- Always `stop` in a `finally` — it's idempotent and always safe. **Never** send
  Hatari's own `hatari-stop`; it deadlocks Hatari 2.4.1 (that's why `pause` is a
  debugger break — ADR-11).
- Long-running tools (`start`, `rip`, `cheat_scan`) can take tens of seconds;
  give your `_recv` a real timeout in anything but a toy client.
- After writing state, `resume` briefly before `screenshot` — a shot taken right
  at the freeze point can capture the prior frame.

### 2. Native MCP mode — register with an agent (use once the binary is stable)

When you're done churning the binary and want first-class tools in Claude Code:

```sh
claude mcp add shirazmcp --env DISPLAY=:0 -- /abs/path/to/target/release/shiraz-mcp
```

- Run it from the repo directory so the default (local) scope attaches to this
  project. Verify with `claude mcp list` (expect `shirazmcp: ✔ Connected`).
- The client binds MCP servers **at startup**, so an already-open session won't
  see the new tools until you `/mcp` reconnect or restart it. They then appear
  as `mcp__shirazmcp__<tool>`.
- Harmless on connect: `unknown format "uint32" ignored …` lines — the client
  just doesn't recognize those JSON-Schema format hints. The server is fine.
- Rebuilt the binary? The running server is stale — `/mcp reconnect` to pick up
  the new one. (This is exactly the friction harness mode avoids mid-dev.)

## Smoke test

The handshake is covered by a real integration test that spawns the compiled
binary over stdio (no Hatari, no display, not `#[ignore]`d):

```sh
cargo test --test mcp_handshake        # initialize + tools/list + status
```

The Hatari-driving integration tests are `#[ignore]`d (they need a display and
the emulator); run a specific one explicitly:

```sh
cargo test --test <name> -- --ignored --nocapture
```

## Debugging

- Server logs (tracing) go to **stderr**. In harness mode, redirect the child's
  stderr to a file and tail it; the JSON-RPC on stdout stays uncontaminated.
- Raw Hatari debugger access is the `debug` tool (one command per call) —
  `evaluate`, `m`, `symbols`, `info ym`, `savebin`, etc. — for anything not yet
  wrapped by a typed tool.
- Hatari's own log/fifo live under `/tmp/shiraz-mcp/` while a session runs.

## House rules (enforced in review + CI-to-be)

`cargo fmt` + `cargo clippy` (warnings denied) + `cargo test` must be green.
Files ≤400 non-test lines, functions ≤30 lines, no `unwrap`/`panic` in library
paths. New non-trivial decisions get an ADR in `decisions.md`; structural
changes update `ARCHITECTURE.md`.

**Pre-commit hook.** A tracked hook at `.githooks/pre-commit` runs the exact CI
gate (fmt-check + clippy `-D warnings` + tests) before every commit. Enable it
once per clone:

```sh
git config core.hooksPath .githooks
```

Skip it for a single commit with `git commit --no-verify`.
