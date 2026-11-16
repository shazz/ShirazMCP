use base64::Engine;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{to_mcp, wrong_state, HatariServer};
use crate::debug;
use crate::error::Error;
use crate::session::HatariProcess;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DebugArgs {
    /// Raw Hatari debugger CLI command, e.g. "evaluate pc", "m $f8000 32",
    /// "b pc=$b00a :once". Runs sentinel-synced.
    pub cmd: String,
    /// Override the default 10000 ms completion timeout.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DebugOutput {
    pub output: String,
    pub truncated: bool,
}

/// Default cap on returned debugger output (chars). Excess is cut at a line
/// boundary and flagged — the agent reruns with a narrower command.
pub const OUTPUT_CAP: usize = 8 * 1024;

#[derive(Debug, Serialize, JsonSchema)]
pub struct RegistersOutput {
    pub d: [String; 8],
    pub a: [String; 8],
    pub usp: String,
    pub isp: String,
    pub pc: String,
    pub sr: SrOutput,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SrOutput {
    pub trace: u8,
    pub supervisor: bool,
    pub x: bool,
    pub n: bool,
    pub z: bool,
    pub v: bool,
    pub c: bool,
    pub imask: u8,
}

/// Debugger output can be arbitrarily large but is always ASCII (Hatari's
/// debugger CLI); a byte-index cut is always on a char boundary.
fn cap_output(output: String) -> (String, bool) {
    if output.len() <= OUTPUT_CAP {
        return (output, false);
    }
    let cut = output[..OUTPUT_CAP].rfind('\n').unwrap_or(OUTPUT_CAP);
    (output[..cut].to_string(), true)
}

pub(crate) fn regs_to_output(r: debug::Registers) -> RegistersOutput {
    let hex = |v: u32| format!("${v:08X}");
    RegistersOutput {
        d: r.d.map(hex),
        a: r.a.map(hex),
        usp: hex(r.usp),
        isp: hex(r.isp),
        pc: r.pc.to_string(),
        sr: SrOutput {
            trace: r.sr.trace,
            supervisor: r.sr.supervisor,
            x: r.sr.x,
            n: r.sr.n,
            z: r.sr.z,
            v: r.sr.v,
            c: r.sr.c,
            imask: r.sr.imask,
        },
    }
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = debugger_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "debug",
        description = "Run one raw Hatari debugger command \
                       (evaluate/m/disasm/b/...) and return its output. \
                       Escape hatch for anything without a dedicated tool. \
                       Output is capped at 8 KiB. Do not arm breakpoints \
                       (`b ...`) via this tool — a firing breakpoint \
                       freezes the machine outside session control; use \
                       the breakpoint tool (action=set) instead."
    )]
    pub async fn debug(
        &self,
        Parameters(args): Parameters<DebugArgs>,
    ) -> Result<Json<DebugOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let timeout = args.timeout_ms.unwrap_or(proc.config().sentinel_timeout_ms);
        let raw = proc
            .debug_with_timeout(&args.cmd, timeout)
            .await
            .map_err(to_mcp)?;
        let (output, truncated) = cap_output(raw);
        Ok(Json(DebugOutput { output, truncated }))
    }

    #[tool(
        name = "registers",
        description = "CPU registers (D0-D7, A0-A7, USP/ISP, PC, status \
                       flags) as typed values."
    )]
    pub async fn registers(&self) -> Result<Json<RegistersOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let regs = debug::registers(proc).await.map_err(to_mcp)?;
        Ok(Json(regs_to_output(regs)))
    }

    #[tool(
        name = "screenshot",
        description = "Capture the emulated screen. Returns the PNG inline \
                       as an image plus the saved file path."
    )]
    pub async fn screenshot(&self) -> Result<CallToolResult, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        take_screenshot(proc, &self.shot_seq).await
    }
}

/// Local patch: session-unique screenshot names (the per-process counter
/// restarts at 0000 every server start, so files from earlier sessions or a
/// parallel worker were silently overwritten / confused).
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn take_screenshot(
    proc: &mut HatariProcess,
    shot_seq: &std::sync::atomic::AtomicU64,
) -> Result<CallToolResult, McpError> {
    let n = shot_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = proc
        .config()
        .work_dir
        .join(format!("screenshot-{}-{n:04}.png", unix_secs()));
    debug::screenshot(proc, &path).await.map_err(to_mcp)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| to_mcp(Error::Io(e)))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(CallToolResult::success(vec![
        ContentBlock::image(b64, "image/png"),
        ContentBlock::text(format!("saved: {}", path.display())),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{Registers, SrFlags};
    use crate::types::Addr;

    #[test]
    fn regs_to_output_includes_trace() {
        let regs = Registers {
            d: [0; 8],
            a: [0; 8],
            usp: 0,
            isp: 0,
            sr: SrFlags {
                trace: 1,
                supervisor: true,
                x: false,
                n: false,
                z: false,
                v: false,
                c: false,
                imask: 0,
            },
            pc: Addr::new(0x1000),
        };
        let out = regs_to_output(regs);
        assert_eq!(out.sr.trace, 1);
        assert_eq!(out.pc, "$001000");
    }
}
