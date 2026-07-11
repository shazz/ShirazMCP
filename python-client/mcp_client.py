"""Minimal Python client for the ShirazMCP server.

Spawns the `shiraz-mcp` binary and speaks line-delimited JSON-RPC 2.0 over stdio
(no Content-Length framing — matches rmcp's `transport::io::stdio()`). This is
all you need to drive the emulator from a script instead of an MCP-aware agent.

    from mcp_client import McpClient
    c = McpClient("target/release/shiraz-mcp", env={"DISPLAY": ":0", **os.environ})
    c.initialize()
    c.call("start", {"harddrive": "/path/to/hd", "machine": "ste"})
    c.screenshot_to("shot.png")
    c.call("stop")

See DEVELOPMENT.md for the protocol details and examples/ for runnable scripts.
"""
from __future__ import annotations

import base64
import json
import selectors
import subprocess
import time
from pathlib import Path
from typing import Any


class McpError(RuntimeError):
    def __init__(self, payload: dict):
        super().__init__(json.dumps(payload))
        self.payload = payload


class McpClient:
    def __init__(self, bin_path: str, env: dict | None = None, log_path: str | None = None):
        self.proc = subprocess.Popen(
            [bin_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=open(log_path, "wb") if log_path else subprocess.DEVNULL,
            env=env,
            bufsize=0,
        )
        self._id = 0
        self._sel = selectors.DefaultSelector()
        self._sel.register(self.proc.stdout, selectors.EVENT_READ)
        self._buf = b""

    def _next_id(self) -> int:
        self._id += 1
        return self._id

    def _send(self, obj: dict) -> None:
        self.proc.stdin.write((json.dumps(obj) + "\n").encode())
        self.proc.stdin.flush()

    def _readline(self, timeout: float) -> bytes | None:
        deadline = time.monotonic() + timeout
        while b"\n" not in self._buf:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            if not self._sel.select(timeout=remaining):
                continue
            chunk = self.proc.stdout.read1(65536)
            if not chunk:
                raise RuntimeError("server closed stdout")
            self._buf += chunk
        line, self._buf = self._buf.split(b"\n", 1)
        return line

    def _recv(self, timeout: float) -> dict:
        line = self._readline(timeout)
        if line is None:
            raise TimeoutError(f"no response within {timeout}s")
        return json.loads(line.decode())

    def initialize(self) -> dict:
        self._send({
            "jsonrpc": "2.0", "id": self._next_id(), "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "python-client", "version": "1.0"}},
        })
        resp = self._recv(15)
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return resp

    def call(self, name: str, args: dict | None = None, timeout: float = 30.0) -> Any:
        rid = self._next_id()
        self._send({"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                    "params": {"name": name, "arguments": args or {}}})
        resp = self._recv(timeout)
        if "error" in resp:
            raise McpError(resp["error"])
        result = resp["result"]
        if result.get("isError"):
            raise McpError(result)
        if result.get("structuredContent") is not None:
            return result["structuredContent"]
        out: dict[str, Any] = {}
        texts = []
        for block in result.get("content", []):
            if block.get("type") == "text":
                texts.append(block["text"])
            elif block.get("type") == "image":
                out["_image_b64"] = block["data"]
        if texts:
            try:
                return json.loads(texts[0])
            except json.JSONDecodeError:
                out["_text"] = "\n".join(texts)
        return out

    def screenshot_to(self, save_path: str, timeout: float = 20.0) -> str:
        res = self.call("screenshot", {}, timeout=timeout)
        b64 = res.get("_image_b64")
        if not b64:
            raise RuntimeError(f"no image in screenshot result: {res}")
        Path(save_path).write_bytes(base64.b64decode(b64))
        return save_path

    def close(self) -> None:
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()
