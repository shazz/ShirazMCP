//! P17 ym_state tool: one non-intrusive read of Hatari's emulated YM2149
//! register file (`info ym`), decoded by `analyze::ym` — the audio sibling
//! of `machine_state`. No freeze: a single debugger round-trip is coherent
//! by construction (the ADR-33 no-freeze decision).

use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::ym_pretty::pretty;
use super::{to_mcp, wrong_state, ym_wire, HatariServer};
use crate::{analyze, debug};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = ym_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "ym_state",
        description = "Decode what the YM2149 sound chip is playing right \
now, from one non-intrusive read of Hatari's emulated register file (the \
$FF8800 select port is never written — the running program is not \
disturbed). Per channel A/B/C: tone period, frequency in Hz, nearest note \
with cent offset, volume 0-15 or envelope mode, and tone/noise enables from \
the mixer; plus noise period, envelope period + shape, and the raw 14 sound \
registers. Works on a running or paused session. Compose with \
identify_music (which player is driving the chip) and spectrogram (what the \
output sounds like)."
    )]
    pub async fn ym_state(&self) -> Result<CallToolResult, McpError> {
        let (session, regs) = {
            let mut guard = self.session.lock().await;
            let name = guard.state_name();
            let proc = guard
                .process_mut()
                .ok_or_else(|| wrong_state(name, "running or paused"))?;
            let regs = debug::ym_registers(proc).await.map_err(to_mcp)?;
            (name, regs)
        };
        let out = ym_wire::build_output(session, &analyze::ym::decode(&regs), &regs);
        let json = serde_json::to_string(&out).map_err(|e| {
            McpError::internal_error(format!("serialize ym_state output: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![
            ContentBlock::text(pretty(&out)),
            ContentBlock::text(json),
        ]))
    }
}
